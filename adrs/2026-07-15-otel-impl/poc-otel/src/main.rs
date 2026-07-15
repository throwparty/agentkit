mod setup;

fn main() {
    let _setup = setup::init_otel().expect("failed to initialise OTel SDK");
    println!("OTel SDK initialised. Placeholder — no signals emitted yet.");
}
