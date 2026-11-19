use std::net::SocketAddr;

use api::config::Config;
use api::routes::auth::create_admin;
use api::{AppState, build_app};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "deskpost", version, about = "Deskpost API server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server.
    Serve,
    /// Apply database migrations.
    Migrate,
    /// Create the first admin user.
    Bootstrap {
        #[arg(long)]
        email: String,
        /// Prefer the environment variable: command-line arguments are visible in `ps`.
        #[arg(long, env = "DP_BOOTSTRAP_PASSWORD", hide_env_values = true)]
        password: String,
        #[arg(long)]
        name: String,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = dp_db::connect(&config.database_url, 10).await?;
    match cli.command {
        Command::Migrate => {
            dp_db::migrate(&pool).await?;
            println!("migrations applied");
        }
        Command::Bootstrap {
            email,
            password,
            name,
        } => {
            let state = AppState::new(pool, config)?;
            let user = create_admin(&state.pool, &state.hasher, &email, &password, &name)
                .await
                .map_err(|e| format!("{}: {}", e.code(), e.message()))?;
            println!("admin created: {}", user.email);
        }
        Command::Serve => {
            let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
            if config.trusted_proxies.is_empty() {
                tracing::warn!(
                    "DP_TRUSTED_PROXIES is empty: behind a reverse proxy every client shares the proxy's address and one rate-limit bucket"
                );
            }
            let state = AppState::new(pool, config)?;
            api::routes::tickets::spawn_presence_gc(state.pool.clone());
            let jobs = api::workers::spawn(state.clone());
            let app = build_app(state);
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!(%addr, "listening");
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown())
            .await?;
            jobs.iter().for_each(|j| j.abort());
            tracing::info!("shut down");
        }
    }
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
