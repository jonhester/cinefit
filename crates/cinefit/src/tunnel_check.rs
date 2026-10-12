//! `cinefit gpu`'s checks of tunneled Dolby Vision: the GPU's unpack of the
//! 12-bit 4:2:2 inside `r210` against the CPU's, bit for bit, and the render
//! through libplacebo's Dolby Vision conversion against the CPU's.
//!
//! The CPU reference shares one constant with libplacebo (the fixed LMS to
//! RGB matrix), so it checks how the conversion is wired: scales, offsets,
//! chroma order and siting. That neutral input stays neutral checks more.

use std::error::Error;
use std::time::Instant;

use cinefit_render::color::dovi12_to_pq_rgb;
use cinefit_render::{DoviColor, Placebo, Primaries, RgbColor, Transfer, tunnel, v210};

/// The conversion a UB820 sends with every tunnel frame (2026-10-12, both
/// saved clips): ICtCp, full range, chroma centred.
const UB820: DoviColor = DoviColor {
    ycc_to_lms: [
        [1.0, 799.0 / 8192.0, 1681.0 / 8192.0],
        [1.0, -933.0 / 8192.0, 1091.0 / 8192.0],
        [1.0, 267.0 / 8192.0, -5545.0 / 8192.0],
    ],
    offset: [0.0, 0.5, 0.5],
    linear: [
        [17081.0 / 16384.0, -349.0 / 16384.0, -349.0 / 16384.0],
        [-349.0 / 16384.0, 17081.0 / 16384.0, -349.0 / 16384.0],
        [-349.0 / 16384.0, -349.0 / 16384.0, 17081.0 / 16384.0],
    ],
};

const OUT: RgbColor = RgbColor {
    primaries: Primaries::Bt2020,
    transfer: Transfer::Pq,
};

pub fn run(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    unpack(pl)?;
    grey(pl)?;
    ramps(pl)
}

/// 12-bit noise from a xorshift64, reproducible.
fn noise(state: &mut u64) -> u16 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 52) as u16
}

fn unpack(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    let mut state = 0x853C_49E6_748F_EA9B_u64;
    let mut failed = false;
    for (w, h, pad) in [
        (3840, 2160, 0),
        (4096, 2160, 0),
        (1920, 1080, 64),
        (2, 1, 0),
    ] {
        let want = v210::Planes {
            width: w,
            height: h,
            luma: (0..w * h).map(|_| noise(&mut state)).collect(),
            chroma: (0..w * h).map(|_| noise(&mut state)).collect(),
        };
        let row_bytes = w * 4 + pad;
        let mut bytes = tunnel::pack_r210(&want, row_bytes).ok_or("tunnel pack")?;
        // r210's unused top bits and each component's two padding bits must
        // not reach the codes.
        for px in bytes.as_chunks_mut::<4>().0 {
            let n = noise(&mut state);
            px[0] |= (n as u8) & 0xC0;
            px[1] |= (n >> 4) as u8 & 0x30;
            px[2] |= (n >> 8) as u8 & 0x0C;
            px[3] |= (n >> 10) as u8 & 0x03;
        }
        let mut planes = pl.planes(w, h)?;
        let started = Instant::now();
        pl.unpack_tunnel(&mut planes, &bytes, row_bytes)?;
        let got = planes.download()?;
        let took = started.elapsed();
        let differ = |want: &[u16], got: &[u16]| {
            want.iter().zip(got).filter(|&(&w, &g)| w << 4 != g).count()
                + want.len().abs_diff(got.len())
        };
        let wrong = differ(&want.luma, &got.luma) + differ(&want.chroma, &got.chroma);
        println!(
            "tunnel unpack {w}x{h} (stride {row_bytes}): {wrong} samples differ, {:.1} ms with upload and readback",
            took.as_secs_f64() * 1000.0
        );
        failed |= wrong > 0;
    }
    if failed {
        return Err("the GPU's tunnel unpack differs from the CPU's".into());
    }
    Ok(())
}

/// Renders `p` (12-bit codes) through the UB820's conversion and reads the
/// PQ RGB back, 0-1.
fn render(pl: &Placebo, p: &v210::Planes) -> Result<Vec<f64>, Box<dyn Error>> {
    let row_bytes = p.width * 4;
    let bytes = tunnel::pack_r210(p, row_bytes).ok_or("tunnel pack")?;
    let mut planes = pl.planes(p.width, p.height)?;
    pl.unpack_tunnel(&mut planes, &bytes, row_bytes)?;
    let target = pl.target_rgba16(p.width, p.height)?;
    pl.render_dovi(&planes, &UB820, target.texture(), OUT)?;
    Ok(target
        .download()?
        .iter()
        .map(|&v| f64::from(v) / 65535.0)
        .collect())
}

/// Every intensity with chroma at the centre: R, G and B must agree, and
/// match the CPU's level. A wrong offset scale or chroma order tints it.
fn grey(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    const TOLERANCE: f64 = 3e-4;
    let (w, h) = (4096, 2);
    let p = v210::Planes {
        width: w,
        height: h,
        luma: (0..h).flat_map(|_| 0..w as u16).collect(),
        chroma: vec![2048; w * h],
    };
    let out = render(pl, &p)?;
    let (mut tint, mut level) = (0f64, 0f64);
    for x in 0..w {
        let rgb = &out[x * 4..x * 4 + 3];
        tint = tint
            .max((rgb[0] - rgb[1]).abs())
            .max((rgb[1] - rgb[2]).abs());
        let want = dovi12_to_pq_rgb(x as f64, 2048.0, 2048.0, &UB820);
        level = level.max((want[1] - rgb[1]).abs());
    }
    println!(
        "tunnel render, neutral 0-4095: largest tint {tint:.1e}, largest difference from the CPU {level:.1e} (tolerance {TOLERANCE:.0e})"
    );
    if tint > TOLERANCE || level > TOLERANCE {
        return Err("neutral tunnel input does not render grey".into());
    }
    Ok(())
}

/// Luma down the rows, Cb rising and Cr falling across the columns by two
/// codes a pixel: every pixel against the CPU's conversion, co-sited, and the
/// centred prediction (a code off) as the control. Steeper ramps measure
/// libplacebo's chroma filter instead: at 16 codes a sample it strayed 5.8e-4
/// between samples while staying within 2.3e-5 on them.
fn ramps(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    // Between co-sited (measured 2.8e-5) and centred (2.1e-4), so the check
    // can fail either way.
    const TOLERANCE: f64 = 1e-4;
    let (w, h) = (128, 32);
    let p = v210::Planes {
        width: w,
        height: h,
        luma: (0..h)
            .flat_map(|y| std::iter::repeat_n(800 + 90 * y as u16, w))
            .collect(),
        // Even pixels hold Cb, odd ones Cr: pair k is (1920 + 4k, 2176 - 4k).
        chroma: (0..h)
            .flat_map(|_| (0..w as u16 / 2).flat_map(|k| [1920 + 4 * k, 2176 - 4 * k]))
            .collect(),
    };
    let out = render(pl, &p)?;
    let (mut cosited, mut centred, mut compared) = (0f64, 0f64, 0usize);
    // On a chroma sample (even) or between two (odd): which one is off says
    // whether the conversion or the upsampling is.
    let mut by_parity = [0f64; 2];
    for y in 0..h {
        for x in 4..w - 4 {
            let luma = f64::from(p.luma[y * w]);
            let predict = |offset: f64| {
                let at = 2.0 * (x as f64 + offset);
                dovi12_to_pq_rgb(luma, 1920.0 + at, 2176.0 - at, &UB820)
            };
            let want = predict(0.0);
            if want.iter().any(|&v| !(0.002..=0.998).contains(&v)) {
                continue;
            }
            let got = &out[(y * w + x) * 4..(y * w + x) * 4 + 3];
            let err = |want: [f64; 3]| (0..3).map(|c| (want[c] - got[c]).abs()).fold(0.0, f64::max);
            cosited = cosited.max(err(want));
            by_parity[x % 2] = by_parity[x % 2].max(err(want));
            centred = centred.max(err(predict(-0.5)));
            compared += 1;
        }
    }
    println!(
        "tunnel render, ramps: {compared} pixels, largest difference {cosited:.1e} from co-sited ({:.1e} on chroma samples, {:.1e} between), {centred:.1e} from centred (tolerance {TOLERANCE:.0e})",
        by_parity[0], by_parity[1]
    );
    if compared < w * h / 4 || cosited > TOLERANCE || centred <= TOLERANCE {
        return Err("the tunnel render differs from the CPU's conversion".into());
    }
    Ok(())
}
