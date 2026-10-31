use clap::{Parser, Subcommand};
use organizer_core::analysis::{self, Settings};
use organizer_core::daw::{self, Daw};
use organizer_core::{PluginFormat, UsageState};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "organizer-cli", about = "Ableton Organizer: command-line front end for the scanning engine")]
struct Cli { #[command(subcommand)] cmd: Cmd }

#[derive(Subcommand)]
enum Cmd {
    /// List DAWs found on this computer
    Detect,
    /// Scan projects, plugins and samples and print a summary
    Scan {
        /// DAWs to take into account: ableton, studioone, flstudio, protools, bitwig, cubase, reaper, logic
        #[arg(long, value_delimiter = ',', default_value = "ableton")] daws: Vec<String>,
        #[arg(long)] projects: Vec<PathBuf>,
        #[arg(long)] samples: Vec<PathBuf>,
        #[arg(long)] plugins: Vec<PathBuf>,
        #[arg(long)] library: Vec<PathBuf>,
        #[arg(long)] cache: Option<PathBuf>,
        #[arg(long, default_value_t = 365)] old_days: u32,
        /// Print the full report as JSON
        #[arg(long)] json: bool,
    },
}

fn parse_daw(s: &str) -> Option<Daw> {
    daw::ALL_DAWS.iter().copied().find(|d| format!("{d:?}").eq_ignore_ascii_case(s) || d.name().replace(' ', "").eq_ignore_ascii_case(s))
}
fn gb(b: u64) -> String { format!("{:.2} GB", b as f64 / 1_073_741_824.0) }
fn mb(b: u64) -> String { format!("{:.1} MB", b as f64 / 1_048_576.0) }

fn main() {
    match Cli::parse().cmd {
        Cmd::Detect => for d in daw::detect() {
            println!("{:<14} {}  {:?}  {}", d.name, if d.installed { "installed" } else { "-        " }, d.support, d.project_dirs.join("; "));
        },
        Cmd::Scan { daws, projects, samples, plugins, library, cache, old_days, json } => {
            let daws: Vec<Daw> = daws.iter().filter_map(|s| parse_daw(s)).collect();
            let s = Settings { daws, project_dirs: projects, sample_dirs: samples, extra_plugin_dirs: plugins, library_dirs: library, old_days, cache_path: cache, preferred_format: PluginFormat::Vst3, ..Settings::default() };
            let a = analysis::run(&s, &|_| {});
            if json { println!("{}", serde_json::to_string_pretty(&a.report).unwrap()); return; }
            let r = &a.report;
            println!("Projects: {}   Plugins: {}   Samples: {} files, {}", r.projects.len(), r.plugins.len(), r.samples.files, gb(r.samples.bytes));
            println!("Time: plugins {} ms, projects {} ms ({} parsed, {} cached), samples {} ms, analysis {} ms\n", r.timings.plugins_ms, r.timings.projects_ms, r.timings.parsed_projects, r.timings.cached_projects, r.timings.samples_ms, r.timings.analysis_ms);
            for p in &r.plugins {
                println!("{:<10} {:<36} used in {:>3} projects  {}", format!("{:?}", p.status), p.name, p.used_projects,
                    p.formats.iter().map(|f| format!("{}{}", f.format.label(), if f.removable { "(removable)" } else { "" })).collect::<Vec<_>>().join(" "));
            }
            println!("\nSamples: used {}, old-only {}, project-owned {}, unused {}", gb(r.samples.used_bytes), gb(r.samples.old_bytes), gb(r.samples.owned_bytes), gb(r.samples.unused_bytes));
            for f in a.samples.top_files(UsageState::Unused, None, 10) { println!("  unused  {:>10}  {}", mb(f.size), f.path); }
            for w in &r.warnings { println!("! {w}"); }
        }
    }
}
