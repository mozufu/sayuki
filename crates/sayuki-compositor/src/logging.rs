use tracing_subscriber::EnvFilter;

pub(crate) fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Err(error) = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init()
    {
        eprintln!("failed to initialize tracing subscriber: {error}");
    }
}
