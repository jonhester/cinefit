//! `cinefit gpu`'s checks of captured frames: for `v210`, `r210` and 8-bit
//! ARGB, the GPU's unpack against the CPU's, bit for bit, and the render
//! from it against the CPU's colour conversion.

use std::error::Error;
use std::time::Instant;

use cinefit_render::color::{rgb_code_to_unit, ycbcr10_to_rgb};
use cinefit_render::{
    Levels, Matrix, Placebo, Primaries, RgbColor, RgbSourceColor, Transfer, YcbcrColor, rgb, v210,
};

pub fn run(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    unfilled(pl)?;
    unpack(pl)?;
    unpack_biased(pl)?;
    render(pl)?;
    unpack_rgb(pl)?;
    render_rgb(pl)
}

/// Textures that no unpack has written hold whatever the allocation left, so
/// reading or rendering them must fail rather than succeed with garbage. The
/// shim logs each refusal.
fn unfilled(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    let planes = pl.planes(8, 2)?;
    let rgb = pl.rgb(8, 2)?;
    let target = pl.target_rgba16(8, 2)?;
    let (pq, bt2020) = (Transfer::Pq, Primaries::Bt2020);
    let out = RgbColor {
        primaries: bt2020,
        transfer: pq,
    };
    let refused = [
        planes.download().is_err(),
        rgb.download().is_err(),
        pl.render_ycbcr(
            &planes,
            YcbcrColor {
                matrix: Matrix::Bt2020Nc,
                primaries: bt2020,
                transfer: pq,
            },
            target.texture(),
            out,
            None,
        )
        .is_err(),
        pl.render_rgb(
            &rgb,
            RgbSourceColor {
                levels: Levels::Limited,
                primaries: bt2020,
                transfer: pq,
            },
            target.texture(),
            out,
            None,
        )
        .is_err(),
    ];
    let n = refused.iter().filter(|&&r| r).count();
    println!("before any unpack: {n} of 4 reads and renders refused (the four log lines above)");
    if n != 4 {
        return Err("a texture no unpack wrote was read or rendered".into());
    }
    Ok(())
}

/// xorshift64: reproducible noise, no dependency.
struct Noise(u64);

impl Noise {
    fn code(&mut self) -> u16 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 54) as u16
    }
}

/// Random codes over all 10 bits, including the 0-3 and 1020-1023 HDMI
/// reserves: the unpack must carry whatever arrives.
fn noise_planes(noise: &mut Noise, width: usize, height: usize) -> v210::Planes {
    let mut p = v210::Planes {
        width,
        height,
        luma: (0..width * height).map(|_| noise.code()).collect(),
        chroma: Vec::new(),
    };
    p.chroma = (0..p.chroma_width() * 2 * height)
        .map(|_| noise.code())
        .collect();
    p
}

/// What the GPU stored against the codes it was given: the samples that
/// differ, and the first of them as (plane, index, expected, got).
fn compare(
    want: &v210::Planes,
    got: &v210::Planes,
) -> (usize, Option<(&'static str, usize, u16, u16)>) {
    let mut wrong = 0;
    let mut first = None;
    for (name, want, got) in [
        ("luma", &want.luma, &got.luma),
        ("chroma", &want.chroma, &got.chroma),
    ] {
        for (i, (&w, &g)) in want.iter().zip(got).enumerate() {
            if w << 6 != g {
                wrong += 1;
                first.get_or_insert((name, i, w << 6, g));
            }
        }
        wrong += want.len().abs_diff(got.len());
    }
    (wrong, first)
}

fn unpack(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
    // (width, height, stride beyond the card's usual 128-byte padding). 3838
    // and 1918 end in a partial group, 7 has an odd pixel with its own chroma.
    let sizes = [
        (3840, 2160, 0),
        (4096, 2160, 0),
        (1920, 1080, 128),
        (3838, 16, 0),
        (1918, 16, 64),
        (7, 4, 0),
        (1, 2, 0),
    ];
    let mut failed = false;
    for (w, h, pad) in sizes {
        let want = noise_planes(&mut noise, w, h);
        let row_bytes = v210::min_row_bytes(w).next_multiple_of(128) + pad;
        let bytes = v210::pack(&want, row_bytes).ok_or("v210 pack")?;
        // Fresh planes for every size, so nothing left from a previous run
        // can pass for this one.
        let mut planes = pl.planes(w, h)?;
        let started = Instant::now();
        pl.unpack_v210(&mut planes, &bytes, row_bytes)?;
        let got = planes.download()?;
        let took = started.elapsed();
        let (wrong, first) = compare(&want, &got);
        print!(
            "v210 unpack {w}x{h} (stride {row_bytes}): {wrong} samples differ, {:.1} ms with upload and readback",
            took.as_secs_f64() * 1000.0
        );
        match first {
            Some((plane, i, want, got)) => {
                println!("; first: {plane} sample {i}, want {want:#06x}, got {got:#06x}");
                failed = true;
            }
            None => println!(),
        }
    }
    if failed {
        return Err("the GPU's v210 unpack differs from the CPU's".into());
    }
    Ok(())
}

/// The unpack with a chroma bias (low-latency Dolby Vision's, #146): luma
/// as without, every Cb and Cr raised by the bias as stored, a fraction of a
/// code rounded to the planes' 16 bits, and the top codes not wrapping.
fn unpack_biased(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    let bias = [0.625f32, 0.40];
    let (w, h) = (1920, 1080);
    let mut want = noise_planes(&mut Noise(0x2545_F491_4F6C_DD1D), w, h);
    want.chroma[0] = 1023;
    want.chroma[1] = 1023;
    let row_bytes = v210::min_row_bytes(w).next_multiple_of(128);
    let bytes = v210::pack(&want, row_bytes).ok_or("v210 pack")?;
    let mut planes = pl.planes(w, h)?;
    pl.unpack_v210_biased(&mut planes, &bytes, row_bytes, Some(bias))?;
    let got = planes.download()?;
    let stored = |code: u16, b: f32| {
        (f64::from(code) * 64.0 + f64::from(b) * 64.0)
            .round()
            .min(65535.0) as u16
    };
    let luma_wrong = (want.luma.iter().zip(&got.luma))
        .filter(|&(&c, &g)| c << 6 != g)
        .count();
    let chroma_wrong = (want.chroma.iter().zip(&got.chroma).enumerate())
        .filter(|&(i, (&c, &g))| stored(c, bias[i % 2]) != g)
        .count();
    let offsets = [
        f64::from(got.chroma[2]) / 64.0 - f64::from(want.chroma[2]),
        f64::from(got.chroma[3]) / 64.0 - f64::from(want.chroma[3]),
    ];
    println!(
        "v210 unpack {w}x{h} with chroma bias {bias:?}: {luma_wrong} luma and {chroma_wrong} chroma samples \
         off, Cb and Cr raised by {:.4} and {:.4} of a code",
        offsets[0], offsets[1]
    );
    if luma_wrong + chroma_wrong > 0 {
        return Err("the v210 unpack's chroma bias is not what was asked".into());
    }
    Ok(())
}

/// Renders ramps through libplacebo into a 16-bit target, colour space
/// unchanged, and compares every pixel with the CPU's conversion. Luma steps
/// down the rows; across the columns Cb rises and Cr falls by one code a
/// pixel, two a chroma sample. That makes chroma siting measurable: co-sited,
/// pixel `x` sees Cb `256 + x`; centred, as libplacebo assumes when not told,
/// `255.5 + x`, half a code off, which is several times the tolerance. Both
/// predictions are reported, so a pass shows the check could tell them apart.
fn render(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    const TOLERANCE: f64 = 3e-4;
    let (w, h) = (512, 64);
    let cw = w / 2;
    let p = v210::Planes {
        width: w,
        height: h,
        luma: (0..h)
            .flat_map(|y| std::iter::repeat_n(100 + 12 * y as u16, w))
            .collect(),
        chroma: (0..h)
            .flat_map(|_| (0..cw as u16).flat_map(|k| [256 + 2 * k, 766 - 2 * k]))
            .collect(),
    };
    let row_bytes = v210::min_row_bytes(w).next_multiple_of(128);
    let bytes = v210::pack(&p, row_bytes).ok_or("v210 pack")?;
    let mut planes = pl.planes(w, h)?;
    pl.unpack_v210(&mut planes, &bytes, row_bytes)?;

    let cases = [
        (
            "BT.709 SDR",
            Matrix::Bt709,
            Primaries::Bt709,
            Transfer::Bt1886,
        ),
        (
            "BT.2020 PQ",
            Matrix::Bt2020Nc,
            Primaries::Bt2020,
            Transfer::Pq,
        ),
    ];
    let mut failed = false;
    for (name, matrix, primaries, transfer) in cases {
        let target = pl.target_rgba16(w, h)?;
        pl.render_ycbcr(
            &planes,
            YcbcrColor {
                matrix,
                primaries,
                transfer,
            },
            target.texture(),
            RgbColor {
                primaries,
                transfer,
            },
            None,
        )?;
        let out = target.download()?;

        let (mut cosited, mut centred, mut compared) = (0f64, 0f64, 0usize);
        for y in 0..h {
            // The ramps' ends, where a filter reaches past the edge, are left
            // out.
            for x in 8..w - 8 {
                let luma = f64::from(p.luma[y * w]);
                let predict = |offset: f64| {
                    let at = x as f64 + offset;
                    ycbcr10_to_rgb(luma, 256.0 + at, 766.0 - at, matrix)
                };
                let want = predict(0.0);
                // Colours outside the target clip; they say nothing.
                if want.iter().any(|&v| !(0.002..=0.998).contains(&v)) {
                    continue;
                }
                let o = (y * w + x) * 4;
                let got = [0, 1, 2].map(|c| f64::from(out[o + c]) / 65535.0);
                let err =
                    |want: [f64; 3]| (0..3).map(|c| (want[c] - got[c]).abs()).fold(0.0, f64::max);
                cosited = cosited.max(err(want));
                centred = centred.max(err(predict(-0.5)));
                compared += 1;
            }
        }
        println!(
            "YCbCr render {name}: {compared} pixels, largest difference {cosited:.1e} from co-sited, {centred:.1e} from centred (tolerance {TOLERANCE:.0e})"
        );
        if compared < w * h / 4 || cosited > TOLERANCE || centred <= TOLERANCE {
            failed = true;
        }
    }
    if failed {
        return Err("the YCbCr render differs from the CPU's conversion".into());
    }
    Ok(())
}

fn unpack_rgb(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    let mut noise = Noise(0x2545_F491_4F6C_DD1D);
    // (width, height, stride beyond four bytes a pixel)
    let sizes = [
        (3840, 2160, 0),
        (4096, 2160, 0),
        (1920, 1080, 128),
        (7, 3, 0),
        (1, 1, 0),
    ];
    let mut failed = false;
    for format in [rgb::Format::R210, rgb::Format::Argb8] {
        let shift = 16 - format.depth();
        for (w, h, pad) in sizes {
            let want = rgb::Rgb {
                width: w,
                height: h,
                format,
                rgb: (0..w * h * 3)
                    .map(|_| noise.code() >> (10 - format.depth()))
                    .collect(),
            };
            let row_bytes = w * 4 + pad;
            let mut bytes = rgb::pack(&want, row_bytes).ok_or("rgb pack")?;
            // Noise where the unpack must not look: r210's two unused bits,
            // ARGB8's alpha, and the padding.
            for b in bytes.as_chunks_mut::<4>().0 {
                b[0] |= match format {
                    rgb::Format::R210 => (noise.code() as u8) & 0xC0,
                    rgb::Format::Argb8 => noise.code() as u8,
                };
            }
            let mut gpu = pl.rgb(w, h)?;
            let started = Instant::now();
            pl.unpack_rgb(&mut gpu, &bytes, row_bytes, format)?;
            let got = gpu.download()?;
            let took = started.elapsed();
            let wrong = want
                .rgb
                .iter()
                .zip(&got)
                .filter(|&(&w, &g)| w << shift != g)
                .count()
                + want.rgb.len().abs_diff(got.len());
            let first = want
                .rgb
                .iter()
                .zip(&got)
                .position(|(&w, &g)| w << shift != g);
            print!(
                "{format:?} unpack {w}x{h} (stride {row_bytes}): {wrong} components differ, {:.1} ms with upload and readback",
                took.as_secs_f64() * 1000.0
            );
            match first {
                Some(i) => {
                    println!(
                        "; first: component {i}, want {:#06x}, got {:#06x}",
                        want.rgb[i] << shift,
                        got[i]
                    );
                    failed = true;
                }
                None if wrong > 0 => {
                    println!();
                    failed = true;
                }
                None => println!(),
            }
        }
    }
    if failed {
        return Err("the GPU's RGB unpack differs from the CPU's".into());
    }
    Ok(())
}

/// Renders ramps over every code of each depth, at both ranges, colour space
/// unchanged, and compares every pixel with the CPU's scaling. The error
/// against the other range is reported too, so a pass shows the check could
/// tell them apart.
fn render_rgb(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    const TOLERANCE: f64 = 3e-4;
    let (w, h) = (1024, 64);
    let mut failed = false;
    for format in [rgb::Format::R210, rgb::Format::Argb8] {
        let depth = format.depth();
        let codes = 1usize << depth;
        let p = rgb::Rgb {
            width: w,
            height: h,
            format,
            rgb: (0..h)
                .flat_map(|y| (0..w).flat_map(move |x| [x, y * 16, x * 7 + y * 3]))
                .map(|c| (c % codes) as u16)
                .collect(),
        };
        let row_bytes = w * 4;
        let bytes = rgb::pack(&p, row_bytes).ok_or("rgb pack")?;
        let mut gpu = pl.rgb(w, h)?;
        pl.unpack_rgb(&mut gpu, &bytes, row_bytes, format)?;
        for (levels, other) in [
            (Levels::Full, Levels::Limited),
            (Levels::Limited, Levels::Full),
        ] {
            let (primaries, transfer) = (Primaries::Bt2020, Transfer::Pq);
            let target = pl.target_rgba16(w, h)?;
            pl.render_rgb(
                &gpu,
                RgbSourceColor {
                    levels,
                    primaries,
                    transfer,
                },
                target.texture(),
                RgbColor {
                    primaries,
                    transfer,
                },
                None,
            )?;
            let out = target.download()?;
            let (mut err, mut err_other, mut compared) = (0f64, 0f64, 0usize);
            for (i, &code) in p.rgb.iter().enumerate() {
                let want = rgb_code_to_unit(code, depth, levels);
                // Limited-range codes past black or white clip; they say
                // nothing.
                if !(0.0..=1.0).contains(&want) {
                    continue;
                }
                let got = f64::from(out[i / 3 * 4 + i % 3]) / 65535.0;
                err = err.max((want - got).abs());
                err_other = err_other.max((rgb_code_to_unit(code, depth, other) - got).abs());
                compared += 1;
            }
            println!(
                "{format:?} render, {levels:?} range: {compared} components, largest difference {err:.1e}, {err_other:.1e} as {other:?} (tolerance {TOLERANCE:.0e})"
            );
            if compared < p.rgb.len() / 2 || err > TOLERANCE || err_other <= TOLERANCE {
                failed = true;
            }
        }
    }
    if failed {
        return Err("the RGB render differs from the CPU's scaling".into());
    }
    Ok(())
}
