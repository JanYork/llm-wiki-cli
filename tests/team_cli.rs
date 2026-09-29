use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn team_server_bootstrap_and_http_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let mut data = temp.path().join("data");
    let initialized = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(temp.path())
        .args(["server", "init", "--data"])
        .arg(&data)
        .args(["--admin-email", "owner@example.com"])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    // Fixture session only: real provider callbacks require deployer credentials.
    let identity: Value = serde_json::from_slice(&initialized.stdout).unwrap();
    let instance_token = fs::read_to_string(data.join("server-access.token")).unwrap();
    let token = "d".repeat(64);
    let hash = |text: &str| {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let control = rusqlite::Connection::open(data.join("control.db")).unwrap();
    control
        .execute(
            "INSERT INTO sessions VALUES(?1,?2,unixepoch()+600)",
            rusqlite::params![hash(&token), identity["user_id"].as_str().unwrap()],
        )
        .unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let origin = format!("http://127.0.0.1:{port}");
    let config = temp.path().join("server.jsonc");
    fs::write(
        &config,
        serde_json::to_vec(
            &json!({"data":data,"listen":format!("127.0.0.1:{port}"),"public_url":origin}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut _server = Server(
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(temp.path())
            .args(["server", "run", "--config"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut gate_headers=reqwest::header::HeaderMap::new();
            gate_headers.insert("x-lwc-server-token",instance_token.parse().unwrap());
            let client = reqwest::Client::builder().default_headers(gate_headers)
                .timeout(Duration::from_secs(2))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(response) = client.get(format!("{origin}/health")).send().await {
                    assert_eq!(response.status(), 200);
                    break;
                }
                assert!(Instant::now() < deadline, "team server failed to start");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(reqwest::Client::new().get(format!("{origin}/api/auth/providers")).send().await.unwrap().status(),401);
            let anonymous=reqwest::Client::new();
            assert_eq!(anonymous.get(format!("{origin}/health")).header("x-lwc-server-token","wrong").send().await.unwrap().status(),401);
            let activated=anonymous.post(format!("{origin}/api/access")).header("Origin",&origin).json(&json!({"token":instance_token})).send().await.unwrap();
            assert_eq!(activated.status(),200);
            let cookie=activated.headers()["set-cookie"].to_str().unwrap().to_owned();
            assert!(cookie.contains("HttpOnly"));assert!(!cookie.contains(&instance_token));
            assert_eq!(anonymous.get(format!("{origin}/api/auth/providers")).header("Cookie",cookie.split(';').next().unwrap()).send().await.unwrap().status(),200);
            let space:Value=client.post(format!("{origin}/api/manage")).bearer_auth(&token)
                .json(&json!({"action":"space.create","team_id":identity["team_id"],"name":"CLI replica"}))
                .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let space_id=space["id"].as_str().unwrap();
            let browser_cookie=format!("{}; lwc_session={}",cookie.split(';').next().unwrap(),token);
            let browser_query=anonymous.post(format!("{origin}/api/spaces/{space_id}/query")).header("Origin",&origin).header("Cookie",&browser_cookie).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap();
            assert_eq!(browser_query.status(),200);
            assert_eq!(anonymous.post(format!("{origin}/api/spaces/{space_id}/query")).header("Origin","https://attacker.example").header("Cookie",&browser_cookie).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap().status(),403);
            let teams:Value=client.get(format!("{origin}/api/admin?view=teams")).bearer_auth(&token).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(teams["rows"].as_array().unwrap().len(),1);
            let project:Value=client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"project.create","team_id":identity["team_id"],"name":"Workspace"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let links=client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"project.spaces","id":project["id"],"items":[space_id],"expected_revision":1})).send().await.unwrap();
            assert_eq!(links.status(),200);
            assert_eq!(control.query_row("SELECT COUNT(*) FROM space_grants WHERE space_id=?1",[space_id],|r|r.get::<_,i64>(0)).unwrap(),1);

            let device:Value=client.post(format!("{origin}/api/auth/device/start")).json(&json!({"name":"UX device"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let approval=json!({"user_code":device["user_code"]});
            assert_eq!(client.post(format!("{origin}/api/auth/device/preview")).header("Origin",&origin).json(&approval).send().await.unwrap().status(),401);
            let preview:Value=client.post(format!("{origin}/api/auth/device/preview")).header("Origin",&origin).bearer_auth(&token).json(&approval).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(preview["name"],"UX device");
            assert!(preview.get("device_code").is_none());
            let pending:Value=client.post(format!("{origin}/api/auth/device/poll")).json(&json!({"device_code":device["device_code"]})).send().await.unwrap().json().await.unwrap();
            assert_eq!(pending["status"],"authorization_pending");
            assert_eq!(client.post(format!("{origin}/api/auth/device/approve")).header("Origin",&origin).bearer_auth(&token).json(&approval).send().await.unwrap().status(),200);
            assert_eq!(client.post(format!("{origin}/api/auth/device/preview")).header("Origin",&origin).bearer_auth(&token).json(&approval).send().await.unwrap().status(),401);
            let invitation:Value=client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"invitation.create","team_id":identity["team_id"],"email":"invitee@example.com"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let preview:Value=client.post(format!("{origin}/api/invitations/preview")).header("Origin",&origin).json(&json!({"invitation_token":invitation["invitation_token"]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert!(preview["team_name"].is_string());
            assert!(preview.get("email").is_none());
            assert_eq!(client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"invitation.accept","invitation_token":invitation["invitation_token"]})).send().await.unwrap().status(),400);

            // Real key login: no injected session and no instance token on this endpoint.
            let opened:Value=client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"member.create","team_id":identity["team_id"],"name":"Key member","days":7})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let personal_key=opened["personal_key"].as_str().unwrap();
            let key_login:Value=anonymous.post(format!("{origin}/api/auth/key")).json(&json!({"key":personal_key,"cli":true})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let key_session=key_login["access_token"].as_str().unwrap();
            let key_user=key_login["user_id"].as_str().unwrap();
            let member_me:Value=anonymous.get(format!("{origin}/api/me")).header("x-lwc-server-token",personal_key).bearer_auth(key_session).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(member_me["spaces"].as_array().unwrap().len(),0);
            let device:Value=client.post(format!("{origin}/api/auth/device/start")).json(&json!({"name":"Key-derived device"})).send().await.unwrap().json().await.unwrap();
            assert_eq!(client.post(format!("{origin}/api/auth/device/approve")).header("Origin",&origin).bearer_auth(key_session).json(&json!({"user_code":device["user_code"]})).send().await.unwrap().status(),200);
            let derived:Value=client.post(format!("{origin}/api/auth/device/poll")).json(&json!({"device_code":device["device_code"]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let derived=derived["access_token"].as_str().unwrap();
            assert_eq!(client.get(format!("{origin}/api/me")).bearer_auth(derived).send().await.unwrap().status(),200);
            assert_eq!(control.query_row("SELECT COUNT(*) FROM identities WHERE user_id=?1",[key_user],|r|r.get::<_,i64>(0)).unwrap(),0);
            let key_space:Value=client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"space.create","team_id":identity["team_id"],"name":"Key delegation"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let key_space=key_space["id"].as_str().unwrap();
            assert_eq!(client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"space.grant","space_id":key_space,"user_id":key_user,"role":"viewer","expected_revision":1})).send().await.unwrap().status(),200);
            let key_agent="c".repeat(64);
            assert_eq!(client.post(format!("{origin}/api/identity/register")).bearer_auth(derived).json(&json!({"email":"key@example.com","nickname":"Key member","device":{"device_id":"b".repeat(64),"os":"test","arch":"test","lwc_version":"test"},"agent_id":key_agent,"agent_name":"Key-derived Agent"})).send().await.unwrap().status(),200);
            let agent:Value=client.post(format!("{origin}/api/agents/delegate")).bearer_auth(derived).json(&json!({"agent_id":key_agent,"space_id":key_space})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let agent=agent["access_token"].as_str().unwrap();
            assert_eq!(client.post(format!("{origin}/api/spaces/{key_space}/query")).bearer_auth(agent).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap().status(),200);
            let key_hash=hash(personal_key);
            assert_eq!(client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"key.revoke","key_id":key_hash})).send().await.unwrap().status(),200);
            assert_eq!(client.post(format!("{origin}/api/spaces/{key_space}/query")).bearer_auth(agent).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap().status(),401);
            for revoked in [key_session,derived] {assert_eq!(client.get(format!("{origin}/api/me")).bearer_auth(revoked).send().await.unwrap().status(),401);}
            assert_eq!(anonymous.get(format!("{origin}/api/me")).header("x-lwc-server-token",personal_key).bearer_auth(key_session).send().await.unwrap().status(),401);
            assert!(!anonymous.post(format!("{origin}/api/auth/key")).json(&json!({"key":personal_key,"cli":true})).send().await.unwrap().status().is_success());
            let administrator_key=fs::read_to_string(identity["administrator_key_file"].as_str().unwrap()).unwrap();
            let browser_login=anonymous.post(format!("{origin}/api/auth/key")).header("Origin",&origin).json(&json!({"key":administrator_key})).send().await.unwrap().error_for_status().unwrap();
            assert_eq!(browser_login.headers().get_all("set-cookie").iter().count(),2);
            let browser_body:Value=browser_login.json().await.unwrap();assert!(browser_body.get("access_token").is_none());

            let home=temp.path().join("home");
            let account=home.join(".lwc/team/accounts").join(hash(&origin));
            fs::create_dir_all(&account).unwrap();
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&account,fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::write(account.join("credentials.json"),serde_json::to_vec(&json!({"server":origin,"user_id":identity["user_id"],"access_token":token})).unwrap()).unwrap();
            let run = |home:&std::path::Path,args:&[&str]| {
                let output=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path())
                    .env("HOME",home).env("USERPROFILE",home).env_remove("LWC_PROJECT_ROOT")
                    .args(args).output().unwrap();
                assert!(output.status.success(),"{:?}: {}",args,String::from_utf8_lossy(&output.stderr));
                serde_json::from_slice::<Value>(&output.stdout).unwrap()
            };
            let configure=|target:&std::path::Path| {
                use std::io::Write;
                let directory=target.join(".lwc/team/accounts").join(hash(&origin));
                fs::create_dir_all(&directory).unwrap();
                #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&directory,fs::Permissions::from_mode(0o700)).unwrap();}
                let mut child=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",target).env("USERPROFILE",target).env_remove("LWC_PROJECT_ROOT")
                    .args(["config","server","--server",&origin,"--token-stdin"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                child.stdin.take().unwrap().write_all(instance_token.as_bytes()).unwrap();
                let output=child.wait_with_output().unwrap();
                assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
                assert!(!String::from_utf8_lossy(&output.stdout).contains(&instance_token));
            };
            configure(&home);
            let cli = |args:&[&str]| run(&home,args);
            let joined=cli(&["space","join",space_id,"--server",&origin,"--manual"]);
            assert_eq!(joined["joined"],true);
            assert_eq!(joined["projection"]["status"],"ready");
            assert_eq!(joined["role"],"manager");
            fs::write(temp.path().join("body.md"),"Local first CLI memory").unwrap();
            cli(&["--space",space_id,"source","add","body.md"]);
            cli(&["--space",space_id,"page","put","local-page","--title","Local page","--file","body.md","--provenance","agent-observed"]);
            let page=cli(&["--space",space_id,"page","show","local-page"]);
            assert_eq!(page["page"]["body"],"Local first CLI memory");
            let replica=account.join("spaces").join(space_id);
            let before_record=fs::read(replica.join("replica.json")).unwrap();
            let backup=temp.path().join("before-sync.db");
            {
                let snapshot=rusqlite::Connection::open(replica.join("wiki.db")).unwrap();
                snapshot.execute("VACUUM INTO ?1",[backup.to_str().unwrap()]).unwrap();
            }
            let synced=cli(&["space","sync",space_id]);
            // A separate credential-only home must remain free of any local memory replica.
            let reader_home=temp.path().join("reader");
            let reader_account=reader_home.join(".lwc/team/accounts").join(hash(&origin));
            fs::create_dir_all(&reader_account).unwrap();
            fs::copy(account.join("credentials.json"),reader_account.join("credentials.json")).unwrap();
            configure(&reader_home);
            let cloud=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"get","local-page"]);
            assert_eq!(cloud["mode"],"remote-read");
            assert_eq!(cloud["data"]["page"]["body"],"Local first CLI memory");
            assert!(cloud["data"]["database"].is_null());
            let listing=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"list","--limit","1"]);
            assert_eq!(listing["data"]["pages"].as_array().unwrap().len(),1);
            let objects=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"objects","--limit","100"]);
            let object=&objects["data"]["objects"][0];
            let fetched=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"object",object["kind"].as_str().unwrap(),object["key"].as_str().unwrap(),"--head","1","--epoch",objects["head"]["server_epoch"].as_str().unwrap()]);
            assert_eq!(fetched["data"]["hash"],object["hash"]);
            assert!(!fetched["data"]["payload"].is_null());
            let blob=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"blob",&hash("Local first CLI memory"),"--limit","5"]);
            assert_eq!(blob["data"]["content"],"TG9jYWw=");
            assert_eq!(blob["data"]["has_more"],true);
            assert_eq!(client.post(format!("{origin}/api/spaces/{space_id}/query")).bearer_auth(&token).json(&json!({"action":"objects","kind":"","limit":1,"offset":0,"head":0,"epoch":objects["head"]["server_epoch"]})).send().await.unwrap().status(),409);

            let found=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"search","Local"]);
            assert!(!found["data"]["results"].as_array().unwrap().is_empty());
            {
                use std::io::Write;
                let mut process=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path())
                    .env("HOME",&reader_home).env("USERPROFILE",&reader_home).env_remove("LWC_PROJECT_ROOT")
                    .args(["serve","--mcp","--path"]).arg(temp.path())
                    .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                writeln!(process.stdin.take().unwrap(),"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lwc_cloud","arguments":{"projectPath":temp.path(),"server":origin,"space":space_id,"query":{"action":"get","slug":"local-page"}}}})).unwrap();
                let output=process.wait_with_output().unwrap();assert!(output.status.success());
                let response:Value=serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(response["result"]["structuredContent"]["data"]["page"]["body"],"Local first CLI memory");
            }
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&reader_account,fs::Permissions::from_mode(0o700)).unwrap();
            }
            let registered=run(&reader_home,&["config","team","--server",&origin,"--email","pretend-admin@example.com","--nickname","Reader","--agent","Temporary agent"]);
            assert_eq!(registered["user_id"],identity["user_id"]);
            assert_eq!(registered["email_verified"],false);
            let registered_again=run(&reader_home,&["config","team","--server",&origin,"--email","owner@example.com","--nickname","Reader renamed","--agent","Temporary agent"]);
            assert_eq!(registered_again["device_id"],registered["device_id"]);
            assert_eq!(registered_again["agent_id"],registered["agent_id"]);
            assert_eq!(registered_again["email_verified"],true);
            assert_eq!(control.query_row("SELECT COUNT(*) FROM devices WHERE user_id=?1",[identity["user_id"].as_str().unwrap()],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(control.query_row("SELECT COUNT(*) FROM agents WHERE user_id=?1",[identity["user_id"].as_str().unwrap()],|r|r.get::<_,i64>(0)).unwrap(),1);
            let delegated_file=temp.path().join("agent-auth/reader.json");
            let delegated=run(&reader_home,&["config","delegate","--server",&origin,"--agent-id",registered["agent_id"].as_str().unwrap(),"--grant-space",space_id,"--output",delegated_file.to_str().unwrap()]);
            assert!(delegated["access_token"].is_null(),"credentials must not reach normal CLI output");
            let secret:Value=serde_json::from_slice(&fs::read(&delegated_file).unwrap()).unwrap();
            let delegated_token=secret["access_token"].as_str().unwrap();
            let agent_home=temp.path().join("agent-only-home");
            configure(&agent_home);
            let delegated_run=|args:&[&str]| Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&agent_home).env("USERPROFILE",&agent_home).env_remove("LWC_PROJECT_ROOT").env("LWC_TEAM_CREDENTIALS_FILE",&delegated_file).args(args).output().unwrap();
            let read=delegated_run(&["cloud","--server",&origin,"--space",space_id,"get","local-page"]);
            assert!(read.status.success(),"{}",String::from_utf8_lossy(&read.stderr));
            assert!(!agent_home.join(".lwc/wiki.db").exists());
            let joined_agent=delegated_run(&["space","join",space_id,"--server",&origin,"--manual"]);
            assert!(joined_agent.status.success(),"{}",String::from_utf8_lossy(&joined_agent.stderr));
            let read_only:Value=serde_json::from_slice(&joined_agent.stdout).unwrap();assert_eq!(read_only["role"],"viewer");
            assert!(std::path::Path::new(read_only["database"].as_str().unwrap()).components().any(|part|part.as_os_str()=="agents"));
            let same_home=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").env("LWC_TEAM_CREDENTIALS_FILE",&delegated_file).args(["space","join",space_id,"--server",&origin,"--manual"]).output().unwrap();
            assert!(same_home.status.success(),"{}",String::from_utf8_lossy(&same_home.stderr));
            let isolated:Value=serde_json::from_slice(&same_home.stdout).unwrap();
            assert_ne!(isolated["database"],joined["database"]);
            assert_eq!(cli(&["space","show",space_id])["replica"]["role"],"manager");
            assert_eq!(client.post(format!("{origin}/api/spaces/{space_id}/push")).bearer_auth(delegated_token).json(&json!({"protocol":"lwc-team-sync/1","share_schema":1,"server_epoch":read_only["head"]["server_epoch"],"expected_head":1,"replica_id":"8".repeat(64),"batch_id":"8".repeat(64),"artifact_id":"8".repeat(64),"payload_digest":"8".repeat(64)})).send().await.unwrap().status(),403);

            let denied=delegated_run(&["--space",space_id,"page","put","forbidden-agent-page","--title","Forbidden","--file","body.md","--provenance","agent-observed"]);
            assert!(!denied.status.success());
            assert_eq!(client.post(format!("{origin}/api/manage")).bearer_auth(delegated_token).json(&json!({"action":"team.create","name":"Escalation"})).send().await.unwrap().status(),401);
            assert_eq!(client.post(format!("{origin}/api/spaces/{}/query","f".repeat(64))).bearer_auth(delegated_token).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap().status(),403);
            assert_eq!(client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"agent.revoke","id":registered["agent_id"]})).send().await.unwrap().status(),200);
            assert!(!delegated_run(&["cloud","--server",&origin,"--space",space_id,"get","local-page"]).status.success());

            assert!(!reader_account.join("spaces").exists());
            assert!(!reader_home.join(".lwc/wiki.db").exists());
            assert_eq!(control.query_row("SELECT COUNT(*) FROM replicas",[],|r|r.get::<_,i64>(0)).unwrap(),2);
            assert_eq!(client.post(format!("{origin}/api/spaces/{space_id}/query")).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap().status(),401);
            assert_eq!(client.post(format!("{origin}/api/spaces/{space_id}/query")).bearer_auth(&token).json(&json!({"action":"delete","slug":"local-page"})).send().await.unwrap().status(),422);
            assert_eq!(client.post(format!("{origin}/api/spaces/{space_id}/query")).bearer_auth(&token).json(&json!({"action":"list","limit":101,"offset":0})).send().await.unwrap().status(),400);
            let hidden="e".repeat(64);
            assert_eq!(client.post(format!("{origin}/api/spaces/{hidden}/query")).bearer_auth(&token).json(&json!({"action":"list","limit":1,"offset":0})).send().await.unwrap().status(),403);

            assert_eq!(synced["status"],"synced");
            assert_eq!(synced["head"],1);
            // Restore the exact client state for a crash after remote commit but before receipt/local publication.
            let pending_root=fs::read_dir(replica.join("staging")).unwrap().map(|e|e.unwrap().path()).find(|p|p.join("pending.json").exists()).unwrap();
            let mut pending:Value=serde_json::from_slice(&fs::read(pending_root.join("pending.json")).unwrap()).unwrap();
            pending["accepted"]=Value::Null;
            fs::write(pending_root.join("pending.json"),serde_json::to_vec(&pending).unwrap()).unwrap();
            for sidecar in ["wiki.db-wal","wiki.db-shm"] {
                let path=replica.join(sidecar);
                if path.exists(){fs::remove_file(path).unwrap();}
            }
            fs::copy(&backup,replica.join("wiki.db")).unwrap();
            fs::write(replica.join("replica.json"),before_record).unwrap();
            fs::write(replica.join("active.json"),serde_json::to_vec(&pending["id"]).unwrap()).unwrap();
            let recovered=cli(&["space","sync",space_id]);
            assert_eq!(recovered["status"],"synced");
            assert_eq!(recovered["head"],1,"receipt recovery must not create another remote commit");
            assert_eq!(cli(&["space","sync",space_id])["status"],"current");
            let repeated=cli(&["space","join",space_id,"--server",&origin,"--manual"]);
            assert_eq!(repeated["replica"]["joined"],true);
            assert_eq!(repeated["database"],joined["database"]);
            let home_b=temp.path().join("home-b");
            let account_b=home_b.join(".lwc/team/accounts").join(hash(&origin));
            fs::create_dir_all(&account_b).unwrap();
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&account_b,fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::copy(account.join("credentials.json"),account_b.join("credentials.json")).unwrap();
            configure(&home_b);
            let b=|args:&[&str]|run(&home_b,args);
            b(&["space","join",space_id,"--server",&origin,"--manual"]);
            assert_eq!(b(&["--space",space_id,"page","show","local-page"])["page"]["body"],"Local first CLI memory");
            for (replica,body) in [(&home,"Evidence from A"),(&home_b,"Evidence from B")] {
                fs::write(temp.path().join("body.md"),body).unwrap();
                run(replica,&["--space",space_id,"page","put","local-page","--title","Local page","--file","body.md","--provenance","agent-observed"]);
            }
            assert_eq!(cli(&["space","sync",space_id])["status"],"synced");
            assert_eq!(b(&["space","sync",space_id])["status"],"conflict");
            let notification=b(&["--space",space_id,"page","list"]);
            assert_eq!(notification["signals"][0]["kind"],"replica.conflict.required");
            let packet=b(&["space","conflicts",space_id]);
            {
                use std::io::Write;
                let mut mcp=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path())
                    .env("HOME",&home_b).env("USERPROFILE",&home_b).env_remove("LWC_PROJECT_ROOT")
                    .args(["serve","--mcp","--path"]).arg(temp.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                let request=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lwc_space","arguments":{"projectPath":temp.path(),"space":space_id,"action":"conflicts"}}});
                let mut stdin=mcp.stdin.take().unwrap();
                writeln!(stdin,"{request}").unwrap();
                for (id,selection) in [(2,Some(space_id)),(3,None)] {
                    let mut arguments=json!({"projectPath":temp.path(),"query":"Evidence","mode":"memory"});
                    if let Some(space)=selection {arguments["space"]=json!(space);}
                    writeln!(stdin,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"lwc_explore","arguments":arguments}})).unwrap();
                }
                drop(stdin);
                let output=mcp.wait_with_output().unwrap();assert!(output.status.success());
                let responses=String::from_utf8(output.stdout).unwrap().lines().map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
                let response=&responses[0];
                let selected:Value=serde_json::from_str(responses[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                assert_eq!(selected["memory"]["state"],"ready");
                assert_eq!(responses[2]["result"]["isError"],true,"space selection must not leak into the next MCP request");
                assert_eq!(response["result"]["structuredContent"]["session"],packet["session"]);
                assert_eq!(response["result"]["structuredContent"]["signals"][0]["kind"],"replica.conflict.required");
            }
            {
                use std::io::Write;
                fs::write(home_b.join(".lwc/update-check.json"),serde_json::to_vec(&json!({"schema":1,"last_attempt":u64::MAX,"latest_version":null,"notified_version":null})).unwrap()).unwrap();
                let mut hook=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path())
                    .env("HOME",&home_b).env("USERPROFILE",&home_b).env_remove("LWC_PROJECT_ROOT")
                    .args(["--space",space_id,"agent","hook","--agent","codex","--event","SessionStart"])
                    .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                writeln!(hook.stdin.take().unwrap(),"{}",json!({"hook_event_name":"SessionStart","cwd":temp.path(),"session_id":"team-space-test","source":"startup"})).unwrap();
                let output=hook.wait_with_output().unwrap();assert!(output.status.success());
                let response:Value=serde_json::from_slice(&output.stdout).unwrap();
                assert!(response["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or("").contains("replica.conflict.required"),"{response}");
            }
            let claim=b(&["space","claim",space_id,"--session",packet["session"].as_str().unwrap(),"--if-digest",packet["digest"].as_str().unwrap()]);
            let contended=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home_b).env("USERPROFILE",&home_b).env_remove("LWC_PROJECT_ROOT").args(["space","claim",space_id,"--session",packet["session"].as_str().unwrap(),"--if-digest",packet["digest"].as_str().unwrap()]).output().unwrap();
            assert!(!contended.status.success());assert!(String::from_utf8_lossy(&contended.stderr).contains("conflict_claimed"));
            assert_eq!(b(&["space","claim",space_id,"--session",packet["session"].as_str().unwrap(),"--if-digest",packet["digest"].as_str().unwrap(),"--claim",claim["claim"].as_str().unwrap()])["claim"],claim["claim"]);
            let claim_path=account_b.join("spaces").join(space_id).join("staging").join(packet["session"].as_str().unwrap()).join("claim.json");
            let mut expired:Value=serde_json::from_slice(&fs::read(&claim_path).unwrap()).unwrap();expired["expires_at"]=json!(0);fs::write(&claim_path,serde_json::to_vec(&expired).unwrap()).unwrap();
            let next_claim=b(&["space","claim",space_id,"--session",packet["session"].as_str().unwrap(),"--if-digest",packet["digest"].as_str().unwrap()]);
            assert_ne!(next_claim["claim"],claim["claim"]);let claim=next_claim;
            let reports:Value=client.get(format!("{origin}/api/admin?view=replicas&scope={space_id}")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
            assert!(reports["rows"].as_array().unwrap().iter().any(|r|r["status"]=="conflict" && r["pending_conflicts"].as_u64().unwrap_or(0)>0),"{reports}");
            let candidate_ref=packet["conflicts"][0]["candidate_refs"][0].as_str().unwrap();
            let candidate=b(&["space","candidate",space_id,"--session",packet["session"].as_str().unwrap(),"--if-digest",packet["digest"].as_str().unwrap(),"--reference",candidate_ref]);
            assert_eq!(candidate["complete"],true);
            let payload:Value=serde_json::from_str(candidate["json_fragment"].as_str().unwrap()).unwrap();
            assert!(matches!(payload["body"].as_str(),Some("Evidence from A"|"Evidence from B")));
            let decisions=packet["conflicts"].as_array().unwrap().iter().map(|c|json!({"conflict_id":c["conflict_id"],"kind":c["kind"],"logical_key":c["logical_key"],"strategy":"preserve_both"})).collect::<Vec<_>>();
            fs::write(temp.path().join("resolution.json"),serde_json::to_vec(&json!({"version":2,"decisions":decisions})).unwrap()).unwrap();
            assert_eq!(b(&["space","resolve",space_id,"--session",packet["session"].as_str().unwrap(),"--if-digest",packet["digest"].as_str().unwrap(),"--file","resolution.json","--claim",claim["claim"].as_str().unwrap()])["status"],"resolved");
            assert_eq!(b(&["space","sync",space_id])["status"],"synced");
            assert_eq!(cli(&["space","sync",space_id])["status"],"synced");
            let pages=cli(&["--space",space_id,"page","list"]);
            let bodies=pages["pages"].as_array().unwrap().iter().map(|p|cli(&["--space",space_id,"page","show",p["slug"].as_str().unwrap()])["page"]["body"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
            assert!(bodies.iter().any(|body|body=="Evidence from A"));
            assert!(bodies.iter().any(|body|body=="Evidence from B"));
            // One autonomous propagation check; no manual sync on the writing replica.
            cli(&["space","configure",space_id,"--interval-ms","250","--automatic","true"]);
            fs::write(temp.path().join("body.md"),"Autonomous propagation").unwrap();
            cli(&["--space",space_id,"page","put","automatic-page","--title","Automatic","--file","body.md","--provenance","agent-observed"]);
            let deadline=Instant::now()+Duration::from_secs(10);
            loop {
                b(&["space","sync",space_id]);
                let pages=b(&["--space",space_id,"page","list"]);
                if pages["pages"].as_array().unwrap().iter().any(|p|p["slug"]=="automatic-page") {break;}
                assert!(Instant::now()<deadline,"worker did not propagate the local write");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            loop {
                let output=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home)
                    .args(["space","configure",space_id,"--automatic","false"]).output().unwrap();
                if output.status.success(){break;}
                assert!(Instant::now()<deadline,"worker configuration remained busy");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let worker_lock=fs::OpenOptions::new().read(true).write(true).open(replica.join("worker.lock")).unwrap();
            while worker_lock.try_lock().is_err() {
                assert!(Instant::now()<deadline,"worker did not stop after disabling automatic sync");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(b(&["--space",space_id,"page","show","automatic-page"])["page"]["body"],"Autonomous propagation");
            // A policy change after an offline edit must reject the actual uploaded diff.
            let accepted=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"get","automatic-page"]);
            fs::write(temp.path().join("body.md"),"Rejected local overwrite").unwrap();
            cli(&["--space",space_id,"page","put","automatic-page","--title","Automatic","--file","body.md","--provenance","agent-observed"]);
            let revision:i64=control.query_row("SELECT revision FROM spaces WHERE id=?1",[space_id],|r|r.get(0)).unwrap();
            let policy=client.post(format!("{origin}/api/manage")).bearer_auth(&token)
                .json(&json!({"action":"space.policy","space_id":space_id,"user_id":identity["user_id"],"expected_revision":revision,"denials":[{"kind":"page","key":"automatic-page","action":"update"}]}))
                .send().await.unwrap();
            assert_eq!(policy.status(),200);
            let refused=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT")
                .args(["space","sync",space_id]).output().unwrap();
            assert!(!refused.status.success(),"policy must reject pending writes despite editor/manager role");
            assert!(String::from_utf8_lossy(&refused.stderr).contains("forbidden"));
            let unchanged=run(&reader_home,&["cloud","--server",&origin,"--space",space_id,"get","automatic-page"]);
            assert_eq!(unchanged["head"],accepted["head"]);
            assert_eq!(unchanged["data"],accepted["data"]);
            assert_eq!(cli(&["--space",space_id,"page","show","automatic-page"])["page"]["body"],"Rejected local overwrite");
            fs::write(temp.path().join("body.md"),"Blocked before local commit").unwrap();
            let local_denied=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT")
                .args(["--space",space_id,"page","put","automatic-page","--title","Automatic","--file","body.md","--provenance","agent-observed"]).output().unwrap();
            assert!(!local_denied.status.success());
            assert!(String::from_utf8_lossy(&local_denied.stderr).contains("replica_permission_denied"));
            assert_eq!(cli(&["--space",space_id,"page","show","automatic-page"])["page"]["body"],"Rejected local overwrite");
            cli(&["--space",space_id,"page","put","allowed-local-page","--title","Allowed","--file","body.md","--provenance","agent-observed"]);
            // Untrusted edits to cached permissions must fail signature validation.
            let local_policy=rusqlite::Connection::open(replica.join("wiki.db")).unwrap();
            let saved:String=local_policy.query_row("SELECT value FROM meta WHERE key='replica_policy'",[],|r|r.get(0)).unwrap();
            let mut altered:Value=serde_json::from_str(&saved).unwrap();
            let mut payload:Value=serde_json::from_str(altered["payload"].as_str().unwrap()).unwrap();payload["denials"]=json!([]);altered["payload"]=json!(payload.to_string());
            local_policy.execute("UPDATE meta SET value=?1 WHERE key='replica_policy'",[altered.to_string()]).unwrap();
            let denied=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).args(["--space",space_id,"page","put","automatic-page","--title","Tampered","--file","body.md","--provenance","agent-observed"]).output().unwrap();
            assert!(!denied.status.success());assert!(String::from_utf8_lossy(&denied.stderr).contains("invalid_policy_signature"));
            local_policy.execute("UPDATE meta SET value=?1 WHERE key='replica_policy'",[saved]).unwrap();


            let providers = client
                .get(format!("{origin}/api/auth/providers"))
                .send()
                .await
                .unwrap();
            assert_eq!(providers.headers()["cache-control"], "no-store");
            assert_eq!(
                providers.json::<Value>().await.unwrap(),
                json!({"email":false,"github":false,"feishu":false})
            );
            assert_eq!(
                client
                    .get(format!("{origin}/api/me"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
            assert_eq!(
                client
                    .post(format!("{origin}/api/manage"))
                    .header("Origin", "https://attacker.example")
                    .json(&json!({"action":"team.create","name":"Cross site"}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                403
            );
            assert_eq!(
                client
                    .post(format!("{origin}/api/auth/email/challenge"))
                    .header("Origin", &origin)
                    .json(&json!({"email":"owner@example.com"}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                503
            );
            let import_space:Value=client.post(format!("{origin}/api/manage")).bearer_auth(&token).json(&json!({"action":"space.create","team_id":identity["team_id"],"name":"Imported"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let import_id=import_space["id"].as_str().unwrap();
            cli(&["space","join",import_id,"--server",&origin,"--manual"]);
            let import_project=temp.path().join("import-project");fs::create_dir(&import_project).unwrap();
            fs::write(import_project.join("note.md"),"Existing local knowledge").unwrap();
            let project_run=|args:&[&str]| {
                let output=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(&import_project).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").args(args).output().unwrap();
                assert!(output.status.success(),"{:?}: {}",args,String::from_utf8_lossy(&output.stderr));
                serde_json::from_slice::<Value>(&output.stdout).unwrap()
            };
            project_run(&["init"]);
            project_run(&["page","put","original","--title","Original","--file","note.md","--provenance","agent-observed"]);
            let bound=project_run(&["space","bind",import_id,"--import-project"]);assert_eq!(bound["bound"],true,"{bound}");
            assert_eq!(project_run(&["page","show","original"])["page"]["body"],"Existing local knowledge");
            assert!(import_project.join(".lwc/wiki.db").is_file());
            {
                use std::io::Write;
                let mut process=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(&import_project).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").args(["serve","--mcp","--path"]).arg(&import_project).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                let mut input=process.stdin.take().unwrap();
                for (id,args) in [(1,json!(["page","put","mcp-write","--title","MCP","--file","note.md","--provenance","agent-observed"])),(2,json!(["page","show","mcp-write"])),(3,json!(["config","show"]))] {
                    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"lwc_core","arguments":{"projectPath":import_project,"args":args}}})).unwrap();
                }
                drop(input);let output=process.wait_with_output().unwrap();assert!(output.status.success());
                let responses=String::from_utf8(output.stdout).unwrap().lines().map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
                assert_ne!(responses[0]["result"]["isError"],true,"{}",responses[0]);
                assert_eq!(responses[1]["result"]["structuredContent"]["page"]["body"],"Existing local knowledge");
                assert_eq!(responses[2]["result"]["isError"],true);
            }

            cli(&["space","sync",import_id]);
            assert_eq!(run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"get","original"])["data"]["page"]["body"],"Existing local knowledge");
            let materialize=cli(&["--space",import_id,"maintenance","materialize"]);
            let work_id=materialize["work"]["id"].as_str().unwrap();
            let terminal=cli(&["--space",import_id,"work","watch",work_id]);
            assert_eq!(terminal["work"]["state"],"succeeded","{terminal}");
            cli(&["space","sync",import_id]);
            let work_audits=run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"objects","--kind","work_audit","--limit","10"]);
            assert!(!work_audits["data"]["objects"].as_array().unwrap().is_empty());
            // Detached core intent must propagate even when live Store identity is unchanged.
            project_run(&["changeset","begin","portable-draft"]);
            project_run(&["--changeset","portable-draft","page","put","draft-page","--title","Draft","--file","note.md","--provenance","agent-observed"]);
            let draft_sync=cli(&["space","sync",import_id]);assert_eq!(draft_sync["status"],"synced");
            let draft_objects=run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"objects","--kind","draft_intent","--limit","10"]);
            assert_eq!(draft_objects["data"]["objects"].as_array().unwrap().len(),1);
            b(&["space","join",import_id,"--server",&origin,"--manual"]);
            let drafts=b(&["--space",import_id,"changeset","list"]);
            assert!(!drafts["changesets"].as_array().unwrap().is_empty(),"{drafts}");
            let stable=run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"objects","--kind","draft_intent","--limit","10"])["head"].clone();
            b(&["space","sync",import_id]);
            assert_eq!(run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"objects","--kind","draft_intent","--limit","10"])["head"],stable);
            // Compensate one bad commit while preserving later edits and a local outbox.
            fs::write(import_project.join("note.md"),"Bad synchronized value").unwrap();
            project_run(&["page","put","original","--title","Original","--file","note.md","--provenance","agent-observed"]);
            let bad=cli(&["space","sync",import_id]);let bad_head=bad["head"].as_u64().unwrap();
            let imported_db=account.join("spaces").join(import_id).join("wiki.db");
            let bad_backup=temp.path().join("bad-before-recovery.db");
            rusqlite::Connection::open(&imported_db).unwrap().execute("VACUUM INTO ?1",[bad_backup.to_str().unwrap()]).unwrap();
            let stale_preview=cli(&["recovery","--server",&origin,"--space",import_id,"--json",&json!({"action":"preview","revert_head":bad_head}).to_string()]);
            fs::write(import_project.join("note.md"),"Later unrelated value").unwrap();
            project_run(&["page","put","mcp-write","--title","MCP","--file","note.md","--provenance","agent-observed"]);
            cli(&["space","sync",import_id]);
            let stale_request=json!({"action":"apply","preview_id":stale_preview["preview_id"],"digest":stale_preview["digest"],"request_id":"8".repeat(64)}).to_string();
            let stale_apply=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").args(["recovery","--server",&origin,"--space",import_id,"--json",&stale_request]).output().unwrap();
            assert!(!stale_apply.status.success());assert!(String::from_utf8_lossy(&stale_apply.stderr).contains("head_changed"));
            fs::write(import_project.join("note.md"),"Preserved local outbox").unwrap();
            project_run(&["page","put","outbox","--title","Outbox","--file","note.md","--provenance","agent-observed"]);
            let preview=cli(&["recovery","--server",&origin,"--space",import_id,"--json",&json!({"action":"preview","revert_head":bad_head}).to_string()]);
            assert_eq!(preview["conflict_count"],0,"{preview}");
            let request=json!({"action":"apply","preview_id":preview["preview_id"],"digest":preview["digest"],"request_id":"9".repeat(64)}).to_string();
            let recovered=cli(&["recovery","--server",&origin,"--space",import_id,"--json",&request]);
            let repeated=cli(&["recovery","--server",&origin,"--space",import_id,"--json",&request]);
            assert_eq!(recovered["team"]["accepted_head"],repeated["team"]["accepted_head"]);
            let history=cli(&["recovery","--server",&origin,"--space",import_id,"--json",&json!({"action":"history","limit":10,"offset":0}).to_string()]);
            assert_eq!(history["history"][0]["recovery"]["revert_head"],bad_head);
            cli(&["space","sync",import_id]);
            assert_eq!(project_run(&["page","show","original"])["page"]["body"],"Existing local knowledge");
            assert_eq!(project_run(&["page","show","mcp-write"])["page"]["body"],"Later unrelated value");
            assert_eq!(project_run(&["page","show","outbox"])["page"]["body"],"Preserved local outbox");
            // Replaying the exact rejected post-image must fail even with a valid writer session.
            project_run(&["page","put","new-local-trigger","--title","Local","--file","note.md","--provenance","agent-observed"]);
            {
                let direct=rusqlite::Connection::open(&imported_db).unwrap();
                direct.execute("ATTACH DATABASE ?1 AS rejected",[bad_backup.to_str().unwrap()]).unwrap();
                direct.execute_batch("DELETE FROM pages WHERE slug='original'; INSERT INTO pages SELECT * FROM rejected.pages WHERE slug='original'; DELETE FROM page_provenance WHERE page_slug='original'; INSERT INTO page_provenance SELECT * FROM rejected.page_provenance WHERE page_slug='original';").unwrap();
            }
            let replay=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(&import_project).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").args(["space","sync",import_id]).output().unwrap();
            assert!(!replay.status.success());assert!(String::from_utf8_lossy(&replay.stderr).contains("revoked_memory_version"),"{}",String::from_utf8_lossy(&replay.stderr));
            assert_eq!(project_run(&["page","list"])["signals"][0]["kind"],"replica.conflict.required");
            let before_retry:i64=control.query_row("SELECT COUNT(*) FROM uploads",[],|r|r.get(0)).unwrap();
            assert_eq!(cli(&["space","sync",import_id])["reason"],"repair_required");
            assert_eq!(control.query_row("SELECT COUNT(*) FROM uploads",[],|r|r.get::<_,i64>(0)).unwrap(),before_retry);
            let rejected=cli(&["space","conflicts",import_id]);assert_eq!(rejected["rejection"]["details"]["code"],"revoked_memory_version");
            fs::write(import_project.join("note.md"),"Agent repaired value").unwrap();
            project_run(&["page","put","original","--title","Original","--file","note.md","--provenance","agent-observed"]);
            let repaired=cli(&["space","sync",import_id]);
            assert_eq!(repaired["reason"],"local_changed","{repaired}");
            assert_eq!(cli(&["space","sync",import_id])["status"],"synced");
            assert_eq!(run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"get","original"])["data"]["page"]["body"],"Agent repaired value");
            let overlap=cli(&["recovery","--server",&origin,"--space",import_id,"--json",&json!({"action":"preview","revert_head":bad_head}).to_string()]);
            assert!(overlap["conflict_count"].as_u64().unwrap()>0,"{overlap}");
            project_run(&["space","unbind"]);
            assert_eq!(project_run(&["page","show","original"])["page"]["body"],"Existing local knowledge");
            // A stopped backup restores into a separate directory; epoch reconciliation keeps offline writes.
            let backup=temp.path().join("backup");
            let live_backup=Command::new(env!("CARGO_BIN_EXE_lwc")).args(["server","backup","--data"]).arg(&data).arg("--output").arg(&backup).output().unwrap();
            assert!(!live_backup.status.success());assert!(String::from_utf8_lossy(&live_backup.stderr).contains("server_running"));
            _server.0.kill().unwrap();_server.0.wait().unwrap();
            let copied=Command::new(env!("CARGO_BIN_EXE_lwc")).args(["server","backup","--data"]).arg(&data).arg("--output").arg(&backup).output().unwrap();
            assert!(copied.status.success(),"{}",String::from_utf8_lossy(&copied.stderr));
            fs::write(temp.path().join("offline.md"),"Survives server restoration").unwrap();
            cli(&["--space",import_id,"page","put","offline-restore","--title","Offline","--file","offline.md","--provenance","agent-observed"]);
            control.execute("UPDATE agents SET revoked=1 WHERE user_id=?1",[identity["user_id"].as_str().unwrap()]).unwrap();
            let restored=temp.path().join("restored");
            let output=Command::new(env!("CARGO_BIN_EXE_lwc")).args(["server","restore","--backup"]).arg(&backup).arg("--authority-data").arg(&data).arg("--output").arg(&restored).output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            let restored_control=rusqlite::Connection::open(restored.join("control.db")).unwrap();
            assert_eq!(restored_control.query_row("SELECT COUNT(*) FROM agents WHERE revoked=0 AND user_id=?1",[identity["user_id"].as_str().unwrap()],|r|r.get::<_,i64>(0)).unwrap(),0);
            data=restored;
            fs::write(&config,serde_json::to_vec(&json!({"data":data,"listen":format!("127.0.0.1:{port}"),"public_url":origin})).unwrap()).unwrap();
            _server=Server(Command::new(env!("CARGO_BIN_EXE_lwc")).args(["server","run","--config"]).arg(&config).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
            let deadline=Instant::now()+Duration::from_secs(10);
            while client.get(format!("{origin}/health")).send().await.is_err(){assert!(Instant::now()<deadline);tokio::time::sleep(Duration::from_millis(50)).await;}
            assert_eq!(cli(&["space","sync",import_id])["status"],"synced");
            assert_eq!(run(&reader_home,&["cloud","--server",&origin,"--space",import_id,"get","offline-restore"])["data"]["page"]["body"],"Survives server restoration");
            let rotated=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).args(["server","rotate-token","--data"]).arg(&data).output().unwrap();
            assert!(rotated.status.success(),"{}",String::from_utf8_lossy(&rotated.stderr));
            assert_eq!(client.get(format!("{origin}/health")).send().await.unwrap().status(),401);
            assert_eq!(anonymous.get(format!("{origin}/health")).header("Cookie",cookie.split(';').next().unwrap()).send().await.unwrap().status(),401);
            let invalidated=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").args(["space","sync",import_id]).output().unwrap();
            assert!(!invalidated.status.success());
            let blocked=Command::new(env!("CARGO_BIN_EXE_lwc")).current_dir(temp.path()).env("HOME",&home).env("USERPROFILE",&home).env_remove("LWC_PROJECT_ROOT").args(["--space",import_id,"page","put","blocked-after-revocation","--title","Blocked","--file","body.md","--provenance","agent-observed"]).output().unwrap();
            assert!(!blocked.status.success());assert!(String::from_utf8_lossy(&blocked.stderr).contains("replica_permission_denied"));
            let replacement=fs::read_to_string(data.join("server-access.token")).unwrap();
            assert_eq!(anonymous.get(format!("{origin}/health")).header("x-lwc-server-token",&replacement).send().await.unwrap().status(),200);
        });
    assert!(
        !temp.path().join(".lwc/wiki.db").exists(),
        "server commands must not initialize a project Wiki"
    );
}
