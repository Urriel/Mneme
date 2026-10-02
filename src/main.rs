use std::path::PathBuf;

use mneme::{Config, DEFAULT_DATA_DIR, DEFAULT_DEVICE, DEFAULT_DIM, DEFAULT_MODEL};

enum Command {
    Init,
    Run,
    Reembed,
    Setup { global: bool },
}

struct Cli {
    command: Command,
    config: Config,
}

fn main() {
    let cli = match parse_args() {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if let Command::Setup { global } = cli.command {
        if let Err(err) = run_setup(global, &cli.config.data_dir) {
            eprintln!("{err}");
            std::process::exit(1);
        }
        return;
    }
    // SAFETY: this thread is the only one alive. The runtime starts after this.
    unsafe {
        std::env::set_var("HF_HUB_DISABLE_PROGRESS_BARS", "1");
    }
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(async {
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .with_max_level(tracing::Level::ERROR)
                .init();
            match cli.command {
                Command::Init => mneme::Mneme::init(&cli.config).await,
                Command::Run => mneme::serve(&cli.config).await,
                Command::Reembed => {
                    let message = mneme::Mneme::reembed_message(&cli.config).await?;
                    eprintln!("{message}");
                    Err(mneme::Error::Store("reembed is not implemented".into()))
                }
            }
        });
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn parse_args() -> Result<Cli, String> {
    let mut args = std::env::args().skip(1);
    let command = match args.next().as_deref() {
        Some("init") => Command::Init,
        Some("run") => Command::Run,
        Some("reembed") => Command::Reembed,
        Some("setup") => {
            return parse_setup(&mut args);
        }
        _ => return Err(usage()),
    };
    let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
    let mut model = DEFAULT_MODEL.to_owned();
    let mut dim = DEFAULT_DIM;
    let mut device = DEFAULT_DEVICE.to_owned();
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(usage)?;
        if value.is_empty() || value.starts_with('-') {
            return Err(usage());
        }
        match arg.as_str() {
            "--data" => data_dir = PathBuf::from(value),
            "--model" => model = value,
            "--dim" => {
                dim = value.parse().map_err(|_| usage())?;
                if dim == 0 {
                    return Err(usage());
                }
            }
            "--device" => device = value,
            _ => return Err(usage()),
        }
    }
    Ok(Cli {
        command,
        config: Config {
            data_dir,
            model,
            dim,
            device,
        },
    })
}

fn parse_setup(args: &mut std::env::Args) -> Result<Cli, String> {
    let mut global = false;
    let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--global" => global = true,
            "--data" => {
                let value = args.next().ok_or_else(usage)?;
                if value.is_empty() || value.starts_with('-') {
                    return Err(usage());
                }
                data_dir = PathBuf::from(value);
            }
            _ => return Err(usage()),
        }
    }
    Ok(Cli {
        command: Command::Setup { global },
        config: Config::new(data_dir),
    })
}

fn run_setup(global: bool, data_dir: &std::path::Path) -> Result<(), mneme::Error> {
    let scope = if global {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| mneme::Error::Store("HOME is not set".into()))?;
        mneme::SetupScope::Global {
            home: PathBuf::from(home),
        }
    } else {
        mneme::SetupScope::Project {
            root: std::env::current_dir()?,
        }
    };
    for line in mneme::setup(scope)? {
        println!("{line}");
    }
    let command = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mneme"));
    println!();
    println!("Add this MCP server in the client. This command does not edit that config.");
    println!("{}", mneme::mcp_snippet(&command, data_dir));
    Ok(())
}

fn usage() -> String {
    "usage: mneme <init|run|reembed|setup> [--data dir] [--model id] [--dim n] [--device cpu] [--global]".into()
}
