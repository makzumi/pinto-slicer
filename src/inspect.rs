//! `--inspect <model or folder>`: load the file(s), do the slicing work once and
//! print what happened and how long it took. Handy for checking a real folder
//! of parts before opening it in the window (and for spotting slow spots).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use crate::config::{default_printers, PrintSettings};
use crate::layout;
use crate::mesh;
use crate::slicer;
use crate::supports::{self, Preset, SupportConfig};

fn models_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext == "stl" || ext == "obj" {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Generate supports for these meshes, with the gap applied first.
fn grow(meshes: &[mesh::Mesh], cfg: &SupportConfig) -> supports::SupportModel {
    let cancel = AtomicBool::new(false);
    let mut lifted: Vec<mesh::Mesh> = meshes.to_vec();
    supports::lift(&mut lifted, cfg.height_from_platform);
    supports::generate(&lifted, cfg, &cancel)
}

pub fn run(path: &Path) -> Result<(), String> {
    let files = if path.is_dir() {
        models_in(path)
    } else {
        vec![path.to_path_buf()]
    };
    if files.is_empty() {
        return Err(format!("no STL or OBJ files in {}", path.display()));
    }

    let printer = default_printers()[0].clone();
    let settings = PrintSettings::default();
    let (mut worst_load, mut worst_slice) = (0u128, 0u128);
    let mut all: Vec<mesh::Mesh> = Vec::new();

    for file in &files {
        let t0 = Instant::now();
        let model = mesh::load_model(file)?;
        let load_ms = t0.elapsed().as_millis();
        worst_load = worst_load.max(load_ms);

        let (min, max) = model.bounds();
        let size = max - min;
        let fits =
            size.x <= printer.build[0] && size.y <= printer.build[1] && size.z <= printer.build[2];

        let t1 = Instant::now();
        let supports = grow(std::slice::from_ref(&model), &settings.supports);
        let mut both = vec![model.clone()];
        supports::lift(&mut both, settings.supports.height_from_platform);
        both.push(supports.mesh(12));
        let slice = slicer::slice(&both, settings.layer_height);
        let slice_ms = t1.elapsed().as_millis();
        worst_slice = worst_slice.max(slice_ms);
        let segs: usize = slice.layers.iter().map(|l| l.segs.len()).sum();
        if std::env::var_os("SUP_DEBUG").is_some() {
            println!("{}", supports.report());
        }

        println!(
            "{:<14} {:>7} tris  {:>6.1} x {:>6.1} x {:>6.1} mm  {}  {:>4} layers  {:>4} tips  {} segs  load {} ms  slice+supports {} ms",
            file.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            model.tris.len(),
            size.x,
            size.y,
            size.z,
            if fits { "fits  " } else { "TOO BIG" },
            slice.layers.len(),
            supports.stats.tips,
            segs,
            load_ms,
            slice_ms,
        );

        all.push(model);
    }

    if all.len() > 1 {
        let bounds: Vec<_> = all.iter().map(|m| m.bounds()).collect();
        let t = Instant::now();
        let slots = layout::arrange(&bounds, printer.build, 6.0);
        println!(
            "\n{} pieces arranged in {} ms on the {} x {} plate:",
            slots.len(),
            t.elapsed().as_millis(),
            printer.build[0],
            printer.build[1]
        );
        let mut outside = 0;
        for slot in &slots {
            let (min, max) = bounds[slot.index];
            let after_min = min + slot.delta;
            let after_max = max + slot.delta;
            let ok = after_min.x >= -printer.build[0] * 0.5 - 0.01
                && after_max.x <= printer.build[0] * 0.5 + 0.01
                && after_min.y >= -printer.build[1] * 0.5 - 0.01
                && after_max.y <= printer.build[1] * 0.5 + 0.01
                && after_min.z.abs() < 0.01;
            if !ok {
                outside += 1;
                println!(
                    "  piece {} does not fit: would sit at x {:.1}..{:.1}, y {:.1}..{:.1}",
                    slot.index, after_min.x, after_max.x, after_min.y, after_max.y
                );
            }
        }
        println!(
            "  {} of {} pieces sit inside the plate",
            slots.len() - outside,
            slots.len()
        );

        // pieces must not land on top of each other
        let mut overlaps = 0;
        for (i, a) in slots.iter().enumerate() {
            let (amin, amax) = bounds[a.index];
            let (amin, amax) = (amin + a.delta, amax + a.delta);
            for b in slots.iter().skip(i + 1) {
                let (bmin, bmax) = bounds[b.index];
                let (bmin, bmax) = (bmin + b.delta, bmax + b.delta);
                let apart_x = amax.x <= bmin.x + 0.01 || bmax.x <= amin.x + 0.01;
                let apart_y = amax.y <= bmin.y + 0.01 || bmax.y <= amin.y + 0.01;
                if !apart_x && !apart_y {
                    overlaps += 1;
                    println!("  piece {} and piece {} overlap", a.index, b.index);
                }
            }
        }
        println!("  {overlaps} overlapping pairs");

        let total: usize = all.iter().map(|m| m.tris.len()).sum();
        let t = Instant::now();
        let supports = grow(&all, &settings.supports);
        let mut both: Vec<mesh::Mesh> = all.clone();
        supports::lift(&mut both, settings.supports.height_from_platform);
        both.push(supports.mesh(12));
        let slice = slicer::slice(&both, settings.layer_height);
        println!(
            "  all together: {} triangles, {} tips, {} layers, {:.1} h estimated, {} ms",
            total,
            supports.stats.tips,
            slice.layers.len(),
            settings.estimated_minutes(slice.layers.len()) / 60.0,
            t.elapsed().as_millis()
        );
        println!("{}", supports.report());
    }

    println!("\nslowest load {worst_load} ms, slowest single model slice {worst_slice} ms");

    // Do the presets really change the result? This is what the Supports tab sets.
    let counts: Vec<String> = [Preset::Light, Preset::Medium, Preset::Heavy]
        .iter()
        .map(|p| {
            let cfg = SupportConfig::from_preset(*p);
            let m = grow(&all, &cfg);
            format!(
                "{} {} tips/{} plates/{} tris",
                p.label(),
                m.stats.tips,
                m.plates.len(),
                m.mesh(12).tris.len()
            )
        })
        .collect();
    println!("supports by preset: {}", counts.join(", "));
    let mut bare = SupportConfig::default();
    bare.tip.shape = supports::TipShape::Cylinder;
    bare.base.plate_thickness = 0.0;
    let m = grow(&all, &bare);
    println!(
        "  plain pins, no plate: {} tips, {} plates, {} triangles",
        m.stats.tips,
        m.plates.len(),
        m.mesh(12).tris.len()
    );

    // How fast can the picture be drawn? This is what the person actually feels.
    let supports = grow(&all, &settings.supports);
    let support_mesh = supports.mesh(12);
    let cam = crate::render::Camera {
        dist: 260.0,
        ..Default::default()
    };
    let Some((device, queue)) = crate::gpu_render::headless_device() else {
        println!("no Vulkan adapter for the GPU benchmark");
        return Ok(());
    };
    let mut gpu = crate::gpu_render::GpuRenderer::new(&device);
    for (w, h) in [(900.0f32, 730.0f32), (300.0, 240.0)] {
        let scene = crate::render::Scene {
            models: &all,
            selection: &[],
            support_mesh: Some(&support_mesh),
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: printer.build,
            mode: crate::render::ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: true,
        };
        let t = Instant::now();
        let frames = 3;
        for _ in 0..frames {
            let view = crate::render::View::new(&cam, w, h);
            gpu.draw(
                &device,
                &queue,
                &scene,
                &view,
                [w as u32, h as u32],
                crate::gpu_render::SceneRevision::default(),
            );
        }
        let _ = device.poll(wgpu::Maintain::Wait);
        println!(
            "  drawing {} triangles at {:.0}x{:.0}: {:.0} ms a frame",
            all.iter().map(|m| m.tris.len()).sum::<usize>(),
            w,
            h,
            t.elapsed().as_millis() as f32 / frames as f32
        );
    }
    // The lighter copies used while dragging, and what they cost to draw.
    let t = Instant::now();
    let coarse: Vec<mesh::Mesh> = all
        .iter()
        .map(|m| {
            let (min, max) = m.bounds();
            m.decimated(((max - min).length() / 300.0).max(0.12))
        })
        .collect();
    let build_ms = t.elapsed().as_millis();
    let scene = crate::render::Scene {
        models: &coarse,
        selection: &[],
        support_mesh: Some(&support_mesh),
        cut_cap: None,
        overhang: None,
        islands: &[],
        build: printer.build,
        mode: crate::render::ViewMode::Solid,
        slice: None,
        current_layer: 0,
        cut: None,
        show_plate: true,
    };
    let frames = 3;
    let t = Instant::now();
    for _ in 0..frames {
        let view = crate::render::View::new(&cam, 495.0, 400.0);
        gpu.draw(
            &device,
            &queue,
            &scene,
            &view,
            [495, 400],
            crate::gpu_render::SceneRevision::default(),
        );
    }
    let _ = device.poll(wgpu::Maintain::Wait);
    println!(
        "  light copies: {} triangles, {} ms to make, {:.0} ms a frame at 495x400",
        coarse.iter().map(|m| m.tris.len()).sum::<usize>(),
        build_ms,
        t.elapsed().as_millis() as f32 / frames as f32
    );
    Ok(())
}
