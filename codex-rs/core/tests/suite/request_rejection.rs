use anyhow::Result;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;

fn user_input(text: &str) -> Op {
    Op::UserInput {
        items: vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_turn_is_not_sent_with_the_next_request() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let requests = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-rejected"),
                json!({
                    "type": "response.failed",
                    "response": {
                        "id": "resp-rejected",
                        "error": {
                            "code": "invalid_prompt",
                            "message": "Request blocked."
                        }
                    }
                }),
            ]),
            sse(vec![
                ev_response_created("resp-recovered"),
                ev_completed("resp-recovered"),
            ]),
        ],
    )
    .await;
    let test = test_codex().build(&server).await?;

    test.codex.submit(user_input("rejected input")).await?;
    let first_complete = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let EventMsg::TurnComplete(first_complete) = first_complete else {
        unreachable!();
    };
    let error = first_complete.error.expect("rejected turn error");
    assert_eq!(error.codex_error_info, Some(CodexErrorInfo::BadRequest));

    test.codex.submit(user_input("recovery input")).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = requests.requests();
    assert_eq!(requests.len(), 2);
    let second_request_user_messages = requests[1].message_input_texts("user");
    assert!(
        second_request_user_messages.contains(&"recovery input".to_string()),
        "second request should contain the recovery input: {second_request_user_messages:?}"
    );
    assert!(
        !second_request_user_messages.contains(&"rejected input".to_string()),
        "second request should exclude the rejected input: {second_request_user_messages:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_turn_with_tool_output_is_not_sent_with_the_next_request() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let call_id = "call-before-rejection";
    let command_args = json!({
        "command": "echo poisoned-tool-output",
    })
    .to_string();
    let requests = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-tool-call"),
                ev_function_call(call_id, "shell_command", &command_args),
                ev_completed("resp-tool-call"),
            ]),
            sse(vec![
                ev_response_created("resp-rejected-after-tool"),
                json!({
                    "type": "response.failed",
                    "response": {
                        "id": "resp-rejected-after-tool",
                        "error": {
                            "code": "invalid_prompt",
                            "message": "Request blocked."
                        }
                    }
                }),
            ]),
            sse(vec![
                ev_response_created("resp-recovered-after-tool"),
                ev_completed("resp-recovered-after-tool"),
            ]),
        ],
    )
    .await;
    let test = test_codex().build(&server).await?;

    test.codex
        .submit(user_input("rejected input with tool"))
        .await?;
    let first_complete = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let EventMsg::TurnComplete(first_complete) = first_complete else {
        unreachable!();
    };
    assert_eq!(
        first_complete
            .error
            .expect("rejected turn error")
            .codex_error_info,
        Some(CodexErrorInfo::BadRequest)
    );

    test.codex.submit(user_input("recovery input")).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = requests.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]
            .function_call_output_text(call_id)
            .is_some_and(|output| output.contains("poisoned-tool-output")),
        "the rejected request should contain the tool output"
    );
    assert_eq!(requests[2].function_call_output_text(call_id), None);
    let recovery_user_messages = requests[2].message_input_texts("user");
    assert!(
        recovery_user_messages.contains(&"recovery input".to_string()),
        "recovery request should contain the recovery input: {recovery_user_messages:?}"
    );
    assert!(
        !recovery_user_messages.contains(&"rejected input with tool".to_string()),
        "recovery request should exclude the rejected input: {recovery_user_messages:?}"
    );

    test.codex.submit(Op::Shutdown).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::ShutdownComplete)
    })
    .await;

    Ok(())
}
