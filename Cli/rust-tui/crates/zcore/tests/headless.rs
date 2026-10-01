//! Contract §7 headless exit codes on the provider path.

use zproto::RunOpts;

#[test]
fn unreachable_provider_exits_4_with_error_event() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("COSMOS_CLI_HOME", home.path());
    std::env::set_var("MERIDIAN_BIN", home.path().join("no-meridian"));
    // a port nothing listens on
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "default_model = \"dead\"\n[providers.dead]\nkind = \"openai\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\n[models.dead]\nprovider = \"dead\"\nid = \"m\"\ncontext = 8000\n"
        ),
    )
    .unwrap();
    let opts = RunOpts { prompt: Some("ping".into()), ..Default::default() };
    let mut kinds = Vec::new();
    let code = zcore::agent::run_headless(&opts, "cosmos", home.path(), &mut |e: &zproto::Event| {
        kinds.push((e.kind.clone(), e.data.clone()))
    })
    .unwrap();
    assert_eq!(code, zproto::exit::PROVIDER);
    let err = kinds.iter().find(|(k, _)| k == "error").expect("error event");
    assert_eq!(err.1["kind"], "provider");
    assert_eq!(kinds.last().unwrap().1["reason"], "provider_error");
}
