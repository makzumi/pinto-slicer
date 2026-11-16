mod batch;
mod branding;
mod config;
mod cxdlp;
mod dialog;
mod export;
mod formats;
mod gizmo;
mod gpu_render;
mod icons;
mod input;
mod inspect;
mod layout;
mod mesh;
mod nav_cube;
mod project;
mod render;
mod selection;
mod slicer;
mod supports;
mod ui;

use std::path::PathBuf;

use eframe::egui;

const USAGE: &str = "Pinto Slicer

Usage:
  pinto_slicer [model.stl ...]    open a window, optionally with models loaded
  pinto_slicer <folder>           open with every STL/OBJ in that folder
  pinto_slicer --preview          start in the sliced-layer view
  pinto_slicer --demo             start with the sample scene instead of an
                                  empty plate
  pinto_slicer --tool move        start with a tool chosen: view, move, turn,
                                  size
  pinto_slicer --screenshot out.png
                                  draw a few frames, save the window as a PNG
                                  and exit (the window does not take focus)
  pinto_slicer --inspect <file|folder>
                                  print what the model file(s) contain and how
                                  long slicing takes, without opening a window
  pinto_slicer --export job.cxdlpv4 <file|folder ...> [--printer NAME]
                                  slice and write at the printer's native
                                  resolution, with its native job format
  pinto_slicer --printer 'ELEGOO Saturn 3 Ultra' --export job.ctb part.stl
                                  select a printer profile for batch export
";

fn main() -> eframe::Result<()> {
    let mut screenshot: Option<PathBuf> = None;
    let mut open: Vec<PathBuf> = Vec::new();
    let mut preview = false;
    let mut demo = false;
    let mut export_to: Option<PathBuf> = None;
    let mut start_tab: Option<String> = None;
    let mut start_tool: Option<String> = None;
    let mut printer_name: Option<String> = None;
    let mut masks = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preview" => preview = true,
            "--demo" => demo = true,
            "--inspect" => match args.next() {
                Some(path) => {
                    if let Err(e) = inspect::run(std::path::Path::new(&path)) {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                    return Ok(());
                }
                None => eprintln!("--inspect needs a file, for example --inspect part.stl"),
            },
            "--export" => match args.next() {
                Some(path) => export_to = Some(PathBuf::from(path)),
                None => {
                    eprintln!("--export needs a job file name, for example --export job.cxdlpv4")
                }
            },
            "--detail" => {
                let _ = args.next();
                eprintln!("--detail is no longer used: printer profiles always export at native resolution");
            }
            "--printer" => {
                if let Some(name) = args.next() {
                    printer_name = Some(name);
                } else {
                    eprintln!("--printer needs a profile name, for example --printer HALOT-X1");
                }
            }
            "--tab" => {
                if let Some(name) = args.next() {
                    start_tab = Some(name);
                }
            }
            "--tool" => {
                if let Some(name) = args.next() {
                    start_tool = Some(name);
                }
            }
            "--no-masks" => masks = false,
            "--screenshot" | "-s" => {
                if let Some(path) = args.next() {
                    screenshot = Some(PathBuf::from(path));
                } else {
                    eprintln!(
                        "--screenshot needs a file name, for example --screenshot shots/now.png"
                    );
                }
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => {
                if !other.starts_with('-') {
                    open.push(PathBuf::from(other));
                }
            }
        }
    }

    // Slicing straight to a file: no window at all.
    if let Some(out) = export_to {
        if let Err(e) = batch::run(&open, &out, masks, printer_name.as_deref()) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(branding::APP_NAME)
        .with_app_id(branding::APP_ID)
        .with_icon(branding::window_icon())
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([980.0, 620.0]);

    if screenshot.is_some() {
        // A check may take the picture while the person is busy elsewhere:
        // open without stealing focus and keep drawing while unfocused.
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    #[cfg(target_os = "macos")]
    let options = {
        let mut options = options;
        if screenshot.is_some() {
            options.event_loop_builder = Some(Box::new(|builder| {
                use winit::platform::macos::EventLoopBuilderExtMacOS;
                builder.with_activate_ignoring_other_apps(false);
            }));
        }
        options
    };

    let shot = screenshot.clone();
    let tab = start_tab.clone();
    let tool = start_tool.clone();
    eframe::run_native(
        branding::APP_NAME,
        options,
        Box::new(move |cc| {
            ui::App::apply_theme(&cc.egui_ctx);
            icons::install(&cc.egui_ctx);
            let mut app = ui::App::new(ui::Launch {
                screenshot: shot.clone(),
                open: open.clone(),
                preview,
                demo,
            });
            if let Some(name) = &tab {
                app.start_on_tab(name);
            }
            if let Some(name) = &tool {
                app.start_with_tool(name);
            }
            Ok(Box::new(app))
        }),
    )
}
