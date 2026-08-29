#![forbid(unsafe_code)]

fn main() {
    if let Err(message) = sippion::cli::run() {
        eprintln!("Sippion: {message}");
        std::process::exit(2);
    }
}
