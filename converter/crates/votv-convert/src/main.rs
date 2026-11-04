//! Command line front end. Same flags as the original `printer_convert.py`.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use clap::Parser;
use votv_convert::{CollisionQuality, ConvertOptions, Hooks, OverwritePolicy, Severity};

#[derive(Parser)]
#[command(
    name = "votv-convert",
    version,
    about = "Convert a GLB into a VotV 3D Printer style folder."
)]
struct Cli {
    /// model file, .glb assumed if no extension
    input: PathBuf,
    /// output name or path, defaults to the input file name in the current dir
    output: Option<PathBuf>,
    /// scale multiplier, glTF is meters and VotV is cm so default 100 converts, use 1 if already cm
    #[arg(long, default_value_t = 100.0)]
    scale: f64,
    /// apply skinned mesh placement using skeleton data
    #[arg(long)]
    humanoid: bool,
    /// max texture size before atlas packing (px)
    #[arg(long, default_value_t = 1024)]
    max_tile_size: u32,
    /// max size of the final atlas (px)
    #[arg(long, default_value_t = 4096)]
    max_atlas_size: u32,
    /// skip auto collision generation
    #[arg(long)]
    no_coll: bool,
    /// ignore UCX_* meshes in the source, generate collision instead
    #[arg(long)]
    force_generate_coll: bool,
    /// (kept for old scripts: a box collider is now always added when nothing else exists)
    #[arg(long, hide = true)]
    fallback_coll: bool,
    #[arg(long, default_value_t = 33.0)]
    fallback_coll_size: f64,
    /// horizontal collision offset (cm)
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    coll_offset_x: f64,
    /// vertical collision offset (cm)
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    coll_offset_y: f64,
    /// max faces per hull, defaults to 200 (high) / 80 (low)
    #[arg(long)]
    max_hull_faces: Option<u32>,
    /// low = single hull, high = VHACD decomposition
    #[arg(long, value_enum, default_value = "high")]
    coll_quality: Quality,
    /// max hulls from high-quality decomposition
    #[arg(long, default_value_t = 64)]
    max_decomp_hulls: u32,
    /// VHACD voxel resolution per axis
    #[arg(long, default_value_t = 64)]
    vhacd_resolution: u32,
    /// don't bake an emissive atlas
    #[arg(long)]
    no_emissive: bool,
    #[arg(long, default_value_t = 10.0)]
    emissive_strength: f64,
    /// impact/footstep sound material 0-15
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=15))]
    physical_material: u8,
    /// don't invert the normal map green channel
    #[arg(long)]
    no_flip_normal_green: bool,
    /// make the print breakable (writes health and the resistances); the default is indestructible
    #[arg(long)]
    breakable: bool,
    /// hit points when breakable, 0 = derived from the size (implies --breakable)
    #[arg(long)]
    health: Option<f64>,
    /// keep the model and add a red copy of the collision (for checking only)
    #[arg(long)]
    show_coll: bool,
    /// drop transparent/masked materials (VotV has no alpha blending)
    #[arg(long)]
    no_transparent: bool,
    /// multiply base colour textures by baseColorFactor (glTF spec; the Python version did not)
    #[arg(long)]
    apply_base_color_factor: bool,
    /// what to do if the output folder exists
    #[arg(long, value_enum, default_value = "replace")]
    overwrite: Overwrite,
    /// write a fresh properties.cfg instead of keeping an existing one
    #[arg(long)]
    no_keep_properties: bool,
    /// also write votv_convert.json (source, options, version)
    #[arg(long)]
    sidecar: bool,
    /// also render icon.png
    #[arg(long)]
    icon: bool,
    #[arg(long, default_value_t = 256)]
    icon_size: u32,
    /// wait for Enter before exiting
    #[arg(long)]
    pause: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Overwrite {
    Replace,
    Fail,
    Rename,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Quality {
    Low,
    High,
}

fn pause() {
    print!("\nPress Enter to close");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

fn main() {
    let color = std::io::stdout().is_terminal();
    let paint = move |code: &str, s: &str| {
        if color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };

    let cli = Cli::parse();
    let (out_dir, out_name) = match &cli.output {
        None => (PathBuf::from("."), None),
        Some(p) => {
            let name = p.file_name().map(|s| s.to_string_lossy().to_string());
            let dir = p
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            (dir, name)
        }
    };
    let _ = cli.fallback_coll;
    let mut opts = ConvertOptions {
        output_dir: out_dir,
        output_name: out_name,
        overwrite: match cli.overwrite {
            Overwrite::Replace => OverwritePolicy::Replace,
            Overwrite::Fail => OverwritePolicy::Fail,
            Overwrite::Rename => OverwritePolicy::Rename,
        },
        keep_existing_properties: !cli.no_keep_properties,
        write_sidecar: cli.sidecar,
        write_icon: cli.icon,
        icon_size: cli.icon_size,
        scale: cli.scale,
        humanoid: cli.humanoid,
        max_tile_size: cli.max_tile_size,
        max_atlas_size: cli.max_atlas_size,
        drop_transparent: cli.no_transparent,
        generate_collision: !cli.no_coll,
        force_generate_collision: cli.force_generate_coll,
        collision_quality: match cli.coll_quality {
            Quality::Low => CollisionQuality::Low,
            Quality::High => CollisionQuality::High,
        },
        max_hull_faces: cli.max_hull_faces,
        max_decomp_hulls: cli.max_decomp_hulls,
        vhacd_resolution: cli.vhacd_resolution,
        fallback_collision_size: cli.fallback_coll_size,
        collision_offset_x: cli.coll_offset_x,
        collision_offset_y: cli.coll_offset_y,
        show_collision: cli.show_coll,
        show_collision_offset: None,
        emissive: !cli.no_emissive,
        flip_normal_green: !cli.no_flip_normal_green,
        apply_base_color_factor: cli.apply_base_color_factor,
        ..Default::default()
    };
    opts.properties.physical_material = cli.physical_material;
    opts.properties.emissive_strength = cli.emissive_strength;
    opts.properties.indestructible = !(cli.breakable || cli.health.is_some());
    if let Some(h) = cli.health {
        opts.properties.health = h;
    }

    let attention = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a2 = attention.clone();
    let hooks = Hooks::default().with_diagnostics(move |d| match d.severity {
        Severity::Info => println!("\x1b[96m{}\x1b[0m", d.message),
        Severity::Warning => println!("\x1b[93m[warning] {}\x1b[0m", d.message),
        Severity::Attention => {
            a2.store(true, std::sync::atomic::Ordering::Relaxed);
            println!(
                "\x1b[91m\x1b[1m[warning]\x1b[0m\x1b[91m {}\x1b[0m",
                d.message
            )
        }
    });

    match votv_convert::convert_with(&opts, &cli.input, &hooks) {
        Ok(r) => {
            println!(
                "{}",
                paint(
                    "92",
                    &format!("[success] Your VotV model is in: {}", r.model_dir.display())
                )
            );
            for f in &r.files {
                println!("  {}  ({:.1} KB)", f.name, f.bytes as f64 / 1024.0);
            }
            if cli.pause || attention.load(std::sync::atomic::Ordering::Relaxed) {
                pause();
            }
        }
        Err(e) => {
            eprintln!("{} {}", paint("91;1", "[error]"), e);
            pause();
            std::process::exit(1);
        }
    }
}
