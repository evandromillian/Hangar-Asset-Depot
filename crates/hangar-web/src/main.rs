//! Hangar web client: Leptos (CSR) UI with an embedded Bevy viewer.

mod api;
mod app;
mod components;
mod pages;
mod upload;
mod util;
mod viewer;

fn main() {
    // A native build (e.g. `cargo run -p hangar-web` or an IDE run config) compiles fine, but
    // every web-sys / js-sys call panics off wasm. Fail early with instructions instead.
    if cfg!(not(target_arch = "wasm32")) {
        eprintln!(
            "hangar-web is a browser (wasm32) app and cannot run natively.\n\
             Run it with Trunk from crates/hangar-web:\n\
             \x20 trunk serve            # dev on http://127.0.0.1:8080, proxies /api -> :8100\n\
             \x20 trunk build --release  # writes dist/, served by hangar-server\n\
             Prerequisites: rustup target add wasm32-unknown-unknown; cargo install trunk --locked"
        );
        std::process::exit(2);
    }

    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
