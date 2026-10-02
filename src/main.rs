use std::path::PathBuf;

use mneme::{Config, DEFAULT_DATA_DIR, DEFAULT_DEVICE, DEFAULT_DIM, DEFAULT_MODEL};

enum Command {
    Init,
    Run,
    Reembed,
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

fn usage() -> String {
    "usage: mneme <init|run|reembed> [--data dir] [--model id] [--dim n] [--device cpu]".into()
}
