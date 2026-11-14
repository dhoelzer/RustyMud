//! Driver entry point — LDMud-style command line.

use ldmud::loader::{Backend, Config};

fn usage() -> ! {
    eprintln!(
        "Usage: ldmud [options] <port>\n\
         Options:\n\
         \x20 -m<dir>, --mudlib <dir>   set mudlib directory\n\
         \x20 -M<file>, --master <file> set master object name\n\
         \x20 -f<flag>                  pass flag to master flag()\n\
         \x20 -N, -e, -d, -T, -Q        (accepted, no-op)\n\
         \x20 -s<file>                  batch script (accepted, no-op)\n\
         \x20 --version                 print version and exit"
    );
    std::process::exit(1);
}

fn main() {
    let mut cfg = Config {
        mudlib_dir: String::new(),
        master_file: "master.c".into(),
        port: 0,
    };
    let mut argv = std::env::args().skip(1);
    let mut flags: Vec<String> = Vec::new();
    while let Some(a) = argv.next() {
        if a == "--version" {
            println!("ldmud-rs 0.1.0 (driver version 30300)");
            return;
        } else if a == "--help" || a == "-h" {
            usage();
        } else if a.starts_with("-m") && a.len() > 2 {
            cfg.mudlib_dir = a[2..].to_string();
        } else if a == "--mudlib" {
            cfg.mudlib_dir = argv.next().unwrap_or_else(|| usage());
        } else if a.starts_with("-M") && a.len() > 2 {
            cfg.master_file = a[2..].to_string();
        } else if a == "--master" {
            cfg.master_file = argv.next().unwrap_or_else(|| usage());
        } else if a.starts_with("-f") && a.len() > 2 {
            flags.push(a[2..].to_string());
        } else if matches!(a.as_str(), "-N" | "-e" | "-d" | "-T" | "-Q") {
            // accepted no-ops
        } else if a.starts_with("-s") {
            // script file: accepted, not implemented yet
        } else if a.parse::<u16>().is_ok() {
            cfg.port = a.parse().unwrap();
        } else {
            eprintln!("Unknown option: {}", a);
            usage();
        }
    }
    if cfg.port == 0 {
        eprintln!("Error: no port specified.");
        usage();
    }
    if cfg.mudlib_dir.is_empty() {
        cfg.mudlib_dir = std::env::current_dir()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into());
    }
    if !cfg.mudlib_dir.ends_with('/') {
        cfg.mudlib_dir.push('/');
    }

    let mut be = Backend::new(cfg);
    if let Err(e) = be.boot_master() {
        eprintln!("Error: Failed to load or initialize the master object.\n{}", e);
        std::process::exit(1);
    }
    for f in flags {
        let r = be.vm.call_function(be.vm.env.master, "flag", &[ldmud::value::mk_string(f)]);
        if let Err(c) = r {
            eprintln!("flag() error: {:?}", c);
        }
        if be.vm.env.shutting_down {
            return;
        }
    }
    if let Err(e) = be.serve() {
        eprintln!("Driver error: {}", e);
        std::process::exit(1);
    }
}
