use std::path::Path;

fn main() {
    let data = match parse_args() {
        Ok(data) => data,
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
            mneme::serve(Path::new(&data)).await
        });
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn parse_args() -> Result<String, String> {
    let mut args = std::env::args().skip(1);
    let mut data = None;
    while let Some(arg) = args.next() {
        if arg != "--data" {
            return Err("usage: mneme --data <dir>".to_string());
        }
        if data.is_some() {
            return Err("usage: mneme --data <dir>".to_string());
        }
        let Some(path) = args.next() else {
            return Err("usage: mneme --data <dir>".to_string());
        };
        if path.is_empty() || path.starts_with('-') {
            return Err("usage: mneme --data <dir>".to_string());
        }
        data = Some(path);
    }
    data.ok_or_else(|| "usage: mneme --data <dir>".to_string())
}
