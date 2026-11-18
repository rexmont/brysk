fn main() {
    if let Err(error) = rf_mac::finder_replace::run_agent() {
        eprintln!("Brysk Finder helper: {error}");
        std::process::exit(1);
    }
}
