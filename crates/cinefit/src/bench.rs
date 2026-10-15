//! `cinefit bench`: what the scanout buffer's layout costs the render, and
//! whether every layout renders the same picture (#101). Off-screen: it
//! allocates buffers as `play` does (GBM, registered with KMS, so the plane
//! is known to take them) in each layout the primary plane lists, renders a
//! 4K frame into each through `play`'s path, and times it.
//!
//! - **Timing:** each render as `play`'s summary measures it, "render, with
//!   the wait for the GPU" (the shim finishes the GPU's work before
//!   returning), after a warm-up: median, p95 and max.
//! - **Correctness:** every buffer read back (GBM's map detiles it) and
//!   compared with LINEAR's, word for word. The C++ version found tiled
//!   bit-identical to LINEAR (cinefit-cpp `scanout_test`).
//! - **A negative control:** a tiled buffer handed to the renderer labelled
//!   LINEAR must come out different, or the comparison could not have seen
//!   a layout mistake.
//!
//! Numbers are clean only with the GPU to itself: the cinefit service
//! stopped, and nothing else running. Off-screen numbers compare layouts;
//! the real pipeline (`play`'s summary) has the last word (AGENTS.md).

use std::error::Error;
use std::os::fd::AsFd;
use std::time::Instant;

use cinefit_kms::Output;
use cinefit_render::{
    DmaBufTarget, Matrix, Placebo, Primaries, Rect, RgbColor, Transfer, YcbcrColor, v210,
};

/// Renders thrown away before timing: shader compiles, clocks rising.
const WARMUP: usize = 20;

/// A frame as the Apple TV sends 4K HDR10: 3840 x 2160 v210, placed into
/// the panel as `play` places it.
const SRC: (usize, usize) = (3840, 2160);

pub fn run(frames: usize, card: Option<&std::path::Path>) -> Result<(), Box<dyn Error>> {
    if service_active() {
        println!(
            "warning: cinefit.service is running; its rendering shares the GPU, so these numbers are not clean \
             (stop it for a clean run)"
        );
    }
    let clocks = cinefit_render::power::StablePstate::acquire();
    match &clocks {
        Ok(p) => println!(
            "gpu clocks: {} on {} (stable pstate held)",
            p.level(),
            p.node()
        ),
        Err(e) => println!("gpu clocks: not held: {e}"),
    }
    let pl = Placebo::new()?;
    println!("device: {}", pl.info().device_name);

    let mut out = Output::open(card)?;
    let modes = out.modes();
    // The panel's size: the largest mode, as the projector's native one is.
    let index = (0..modes.len())
        .max_by_key(|&i| u32::from(modes[i].width) * u32::from(modes[i].height))
        .ok_or("the display offers no modes")?;
    let (w, h) = (
        usize::from(modes[index].width),
        usize::from(modes[index].height),
    );
    let mut modifiers = out.scanout_modifiers()?;
    // LINEAR first: everything is compared with it.
    modifiers.sort_by_key(|&m| m != 0);
    modifiers.dedup();
    if modifiers.first() != Some(&0) {
        modifiers.insert(0, 0);
    }

    // The frame: a ramp across, steps down, chroma sweeping, so a tile in the
    // wrong place shows.
    let (sw, sh) = SRC;
    let p = v210::Planes {
        width: sw,
        height: sh,
        luma: (0..sh)
            .flat_map(|y| (0..sw).map(move |x| 64 + ((x * 876 / sw + y / 8) % 877) as u16))
            .collect(),
        chroma: (0..sh)
            .flat_map(|y| {
                (0..sw / 2).flat_map(move |k| {
                    [
                        (64 + (k + y) % 897) as u16,
                        (960 - (k * 3 + y) % 897) as u16,
                    ]
                })
            })
            .collect(),
    };
    let row_bytes = v210::min_row_bytes(sw).next_multiple_of(128);
    let bytes = v210::pack(&p, row_bytes).ok_or("v210 pack")?;
    let mut planes = pl.planes(sw, sh)?;
    let t0 = Instant::now();
    pl.unpack_v210(&mut planes, &bytes, row_bytes)?;
    println!(
        "source {sw}x{sh} v210, unpacked once ({:.1} ms with upload); target {w}x{h}, {} frames each after {WARMUP} warm-up",
        ms(t0),
        frames
    );
    let color = YcbcrColor {
        matrix: Matrix::Bt2020Nc,
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let dst_color = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let place = Some(Rect::fit(sw as u32, sh as u32, w as u32, h as u32));

    println!();
    println!(
        "{:<20} {:>8} {:>8} {:>8} {:>8}  vs LINEAR",
        "layout", "median", "p95", "max", "stride"
    );
    let mut linear: Option<Vec<u32>> = None;
    let mut tiled_frame = None;
    let mut bad = 0;
    for &m in &modifiers {
        let name = cinefit_kms::modifier_name(m);
        let frame = match out.create_scanout_as(index, m) {
            Ok(f) => f,
            Err(e) => {
                println!("{name:<20} not allocated: {e}");
                continue;
            }
        };
        let buf = out.export(frame)?;
        let target = match pl.import_dmabuf(&DmaBufTarget {
            fd: buf.fd.as_fd(),
            width: buf.width,
            height: buf.height,
            fourcc: buf.fourcc,
            modifier: buf.modifier,
            stride: buf.stride,
            offset: buf.offset,
        }) {
            Ok(t) => t,
            Err(e) => {
                println!("{name:<20} not imported by the renderer: {e}");
                continue;
            }
        };
        let mut times = Vec::with_capacity(frames);
        for i in 0..WARMUP + frames {
            let t = Instant::now();
            pl.render_ycbcr(&planes, color, &target, dst_color, place)?;
            if i >= WARMUP {
                times.push(ms(t));
            }
        }
        times.sort_by(f64::total_cmp);
        let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
        let words = read_words(&out, frame, w, h)?;
        let compared = match &linear {
            None => {
                linear = Some(words);
                "(the reference)".to_string()
            }
            Some(l) => {
                let differ = l.iter().zip(&words).filter(|(a, b)| a != b).count();
                if differ > 0 {
                    bad += 1;
                }
                if tiled_frame.is_none() {
                    tiled_frame = Some((frame, buf.stride, m));
                }
                format!("{differ} of {} pixels differ", w * h)
            }
        };
        println!(
            "{name:<20} {:>6.2}ms {:>6.2}ms {:>6.2}ms {:>8}  {compared}",
            at(0.5),
            at(0.95),
            times[times.len() - 1],
            buf.stride
        );
    }

    // The negative control: a tiled buffer labelled LINEAR to the renderer.
    let Some(reference) = linear else {
        return Err("LINEAR could not be rendered: nothing to compare with".into());
    };
    println!();
    match tiled_frame {
        None => println!("negative control: no tiled layout to mislabel"),
        Some((frame, _, m)) => {
            let buf = out.export(frame)?;
            let lie = DmaBufTarget {
                fd: buf.fd.as_fd(),
                width: buf.width,
                height: buf.height,
                fourcc: buf.fourcc,
                modifier: 0,
                stride: (w * 4) as u32,
                offset: 0,
            };
            match pl.import_dmabuf(&lie) {
                Err(e) => println!(
                    "negative control: the renderer refused {} labelled LINEAR ({e}); the check is unproven",
                    cinefit_kms::modifier_name(m)
                ),
                Ok(target) => {
                    pl.render_ycbcr(&planes, color, &target, dst_color, place)?;
                    let words = read_words(&out, frame, w, h)?;
                    let differ = reference.iter().zip(&words).filter(|(a, b)| a != b).count();
                    println!(
                        "negative control: {} labelled LINEAR, {differ} of {} pixels differ{}",
                        cinefit_kms::modifier_name(m),
                        w * h,
                        if differ == 0 {
                            " -- the comparison cannot see a layout mistake"
                        } else {
                            " (as it must)"
                        }
                    );
                    if differ == 0 {
                        return Err("the negative control saw no difference".into());
                    }
                }
            }
        }
    }
    drop(clocks);
    if bad > 0 {
        return Err(format!("{bad} layouts render differently from LINEAR").into());
    }
    Ok(())
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// A scanout buffer's pixels as words, rows packed; GBM's map detiles.
fn read_words(out: &Output, frame: usize, w: usize, h: usize) -> Result<Vec<u32>, Box<dyn Error>> {
    Ok(out.read_scanout(frame, |px, stride| {
        let mut words = Vec::with_capacity(w * h);
        for y in 0..h {
            let row = &px[y * stride..y * stride + w * 4];
            words.extend(
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_le_bytes(*b)),
            );
        }
        words
    })?)
}

/// Whether the cinefit service is running, which would share the GPU.
fn service_active() -> bool {
    std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "cinefit"])
        .status()
        .is_ok_and(|s| s.success())
}
