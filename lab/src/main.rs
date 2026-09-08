use axum::{Router, http::StatusCode, routing::get};
use tokio::net::TcpListener;

async fn unit_handler() {
    println!("[ INFO ] Request received");
}

#[tokio::main]
async fn main() {
    let listener: TcpListener;
    loop {
        match tokio::net::TcpListener::bind("127.0.0.1:3000").await {
            Err(e) => {
                // Will never end unless the socket is created.
                println!("[ ERROR ] Cannot create the listener.");
                println!("{e:?}");
            }
            Ok(l) => {
                listener = l;
                break;
            }
        }
    }
    let app = Router::new()
        .route("/clients/auth/login", get(unit_handler))
        .route("/clients/home", get(unit_handler))
        .route("/clients/submit-ticket", get(unit_handler))
        .route("/clients/exchange-points", get(unit_handler));
    match axum::serve(listener, app).await {
        Ok(_) => {}
        Err(e) => {
            println!("{e:?}");
        }
    }
}
