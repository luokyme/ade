//! Runs against a real T3 Code server. Set `T3_LIVE_HOME` to its data directory and
//! `T3_LIVE_TOKEN_FILE` to where the token may be kept, then run with `--ignored`.

use serde_json::Value;
use std::path::PathBuf;
use t3_client::{
    ConnectOptions, Connection,
    local::{LocalServer, TokenFile},
    protocol::{ShellStreamItem, ThreadStreamItem},
    state::{ShellState, ThreadState},
};

fn options() -> Option<ConnectOptions> {
    let home = std::env::var_os("T3_LIVE_HOME")?;
    let token_file = std::env::var_os("T3_LIVE_TOKEN_FILE")?;
    Some(ConnectOptions {
        server: LocalServer {
            home: PathBuf::from(home),
            executable: t3_client::local::find_executable(),
        },
        token_file: TokenFile::new(PathBuf::from(token_file)),
        app_version: "test".into(),
    })
}

async fn first_value(subscription: &mut t3_client::rpc::Subscription) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(20), subscription.next())
        .await
        .expect("a value in time")
        .expect("stream open")
        .expect("no error")
}

#[tokio::test]
#[ignore]
async fn reads_the_shell_and_a_thread_from_a_live_server() {
    let Some(options) = options() else {
        eprintln!("T3_LIVE_HOME and T3_LIVE_TOKEN_FILE are not set");
        return;
    };
    let connection = Connection::connect_local(&options).await.expect("connects");
    assert_eq!(
        connection.config.environment.orchestration_protocol_version,
        2
    );
    assert!(
        connection
            .config
            .providers
            .iter()
            .any(|provider| provider.is_ready()),
        "no provider is ready"
    );

    let mut shell_stream = connection.subscribe_shell().expect("subscribes");
    let mut shell = ShellState::default();
    shell.apply(ShellStreamItem::parse(&first_value(&mut shell_stream).await).expect("parses"));
    let project = shell.projects.first().expect("a project").clone();
    let thread = shell
        .project_threads(&project.id)
        .first()
        .map(|thread| thread.id.clone())
        .expect("a thread");

    let mut thread_stream = connection.subscribe_thread(&thread).expect("subscribes");
    let mut state = ThreadState::new(thread.clone());
    state.apply(ThreadStreamItem::parse(&first_value(&mut thread_stream).await).expect("parses"));
    let kinds = state
        .timeline()
        .map(|item| item.kind.clone())
        .collect::<Vec<_>>();
    eprintln!("{thread}: {kinds:?}");
    assert!(kinds.iter().any(|kind| kind == "user_message"));
    assert!(kinds.iter().any(|kind| kind == "assistant_message"));
}
