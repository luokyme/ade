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

/// Starts a thread in `T3_LIVE_PROJECT` with `T3_LIVE_PROMPT`, follows it until the run settles
/// or waits on the user, and reads back any output the stream left out.
#[tokio::test]
#[ignore]
async fn launches_a_thread_and_reads_omitted_output() {
    let (Some(options), Some(root), Some(prompt)) = (
        options(),
        std::env::var_os("T3_LIVE_PROJECT"),
        std::env::var("T3_LIVE_PROMPT").ok(),
    ) else {
        eprintln!("T3_LIVE_HOME, T3_LIVE_TOKEN_FILE, T3_LIVE_PROJECT and T3_LIVE_PROMPT are not set");
        return;
    };
    let root = PathBuf::from(root);
    let connection = Connection::connect_local(&options).await.expect("connects");
    let mut shell_stream = connection.subscribe_shell().expect("subscribes");
    let mut shell = ShellState::default();
    shell.apply(ShellStreamItem::parse(&first_value(&mut shell_stream).await).expect("parses"));
    let project_id = match shell.project_for_root(&root) {
        Some(project) => project.id.clone(),
        None => connection
            .create_project("t3play", &root.to_string_lossy())
            .await
            .expect("creates the project"),
    };
    let provider = connection
        .config
        .providers
        .iter()
        .find(|provider| provider.is_ready())
        .expect("a ready provider");
    let selection = t3_client::ModelSelection {
        instance_id: provider.instance_id.clone(),
        model: provider.default_model().expect("a model").slug.clone(),
        options: None,
    };
    let input = t3_client::protocol::commands::launch_thread(
        &project_id,
        &selection,
        "full-access",
        "default",
        &prompt,
        &[],
    );
    let thread = connection.launch_thread(input).await.expect("launches");
    let mut thread_stream = connection.subscribe_thread(&thread).expect("subscribes");
    let mut state = ThreadState::new(thread.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        let next = tokio::time::timeout(std::time::Duration::from_secs(60), thread_stream.next())
            .await
            .expect("an event in time")
            .expect("stream open")
            .expect("no error");
        state.apply(ThreadStreamItem::parse(&next).expect("parses"));
        let started = state.runs.iter().any(|run| run.started_at.is_some());
        let waiting = state.runtime_requests.iter().any(|request| request.is_pending());
        if (started && state.active_run().is_none()) || waiting {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the run did not settle");
    }
    let kinds = state
        .timeline()
        .map(|item| item.kind.clone())
        .collect::<Vec<_>>();
    eprintln!("{thread}: {kinds:?}");
    for item in state.timeline().filter(|item| item.bool_field("outputOmitted")) {
        let full = connection
            .get_turn_item(&thread, &item.id)
            .await
            .expect("fetches the item")
            .expect("the item exists");
        eprintln!(
            "{} {}: {} bytes of output",
            item.kind,
            item.id,
            full.str_field("output").map_or(0, str::len)
        );
        assert_eq!(full.id, item.id);
    }
}

/// Uploads an image and downloads it back through an asset ticket, as the thread view does for
/// images in sent messages.
#[ignore]
#[tokio::test]
async fn downloads_an_uploaded_attachment() {
    let Some(options) = options() else {
        eprintln!("T3_LIVE_HOME and T3_LIVE_TOKEN_FILE are not set");
        return;
    };
    let connection = Connection::connect_local(&options).await.expect("connecting");
    let png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0, 0,
        0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0d, b'I', b'D', b'A',
        b'T', 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35,
        0x81, 0x84, 0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82,
    ];
    let attachment = connection
        .upload_attachment("dot.png", png.clone())
        .await
        .expect("uploading");
    let bytes = connection
        .download_attachment(&attachment.id, &attachment.name, &attachment.mime_type)
        .await
        .expect("downloading");
    assert_eq!(bytes, png);
}

#[tokio::test]
#[ignore]
async fn card_answers_reach_the_server() {
    let Some(options) = options() else {
        eprintln!("T3_LIVE_HOME and T3_LIVE_TOKEN_FILE are not set");
        return;
    };
    let connection = Connection::connect_local(&options).await.expect("connects");
    let error = connection
        .answer_secret_request(
            "missing-thread",
            "missing-item",
            &t3_client::protocol::SecretAnswer::Decline,
        )
        .await
        .expect_err("there is no such card");
    let t3_client::rpc::RpcError::Failure { error, .. } = error else {
        panic!("expected a typed failure, got {error}");
    };
    assert_eq!(error["_tag"], "SecretRequestError", "{error}");

    let error = connection
        .answer_feedback_request("missing-thread", "missing-item", false, false)
        .await
        .expect_err("there is no such card");
    let t3_client::rpc::RpcError::Failure { error, .. } = error else {
        panic!("expected a typed failure, got {error}");
    };
    assert_eq!(error["_tag"], "AgentFeedbackRequestError", "{error}");

    let error = connection
        .answer_link_request(
            "missing-thread",
            "missing-item",
            &t3_client::protocol::LinkAnswer::Decline,
        )
        .await
        .expect_err("there is no such card");
    let t3_client::rpc::RpcError::Failure { error, .. } = error else {
        panic!("expected a typed failure, got {error}");
    };
    assert_eq!(error["_tag"], "PeerLinkRequestError", "{error}");
}
