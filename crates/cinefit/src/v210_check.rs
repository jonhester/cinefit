//! `cinefit gpu`'s checks of captured YCbCr: the GPU's `v210` unpack against
//! the CPU's, bit for bit, and the render from its planes against the CPU's
//! colour conversion.

use std::error::Error;
use std::time::Instant;

use cinefit_render::color::ycbcr10_to_rgb;
use cinefit_render::{Matrix, Placebo, Primaries, RgbColor, Transfer, YcbcrColor, v210};

pub fn run(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    unpack(pl)?;
    render(pl)
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
