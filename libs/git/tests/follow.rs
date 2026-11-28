use makepad_git::follow::{self, FetchFrom, Plan, Upstream};
use makepad_git::test_support::{build_pack_bytes, tempdir, PackEntry};
use makepad_git::{config, GitHttpMethod, GitHttpResponse, NoopHttpSyncHooks, ObjectId, RefTarget, Repository, Signature, StatusOptions};
use std::fs;
use std::path::Path;

fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    Repository::open(dir).unwrap()
}

fn commit(repo: &mut Repository, message: &str, edits: &[(&str, Option<&str>)]) -> ObjectId {
    for &(name, contents) in edits {
        let file = repo.workdir.join(name);
        if let Some(contents) = contents {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, contents).unwrap();
            repo.stage_file(name).unwrap();
        } else {
            fs::remove_file(file).unwrap();
            repo.unstage_file(name).unwrap();
        }
    }
    let author = Signature { name: "Test".into(), email: "test@example.org".into(), timestamp: 0, tz_offset: "+0000".into() };
    repo.commit(message, author).unwrap()
}

fn local_fetch(repo: &mut Repository, up: &Upstream) -> follow::Fetched {
    follow::fetch(repo, up, &mut |_| panic!("local fetch must use only the object store"), &mut NoopHttpSyncHooks).unwrap()
}

#[test]
fn follows_only_the_configured_branch_and_protects_local_work() {
    let tmp = tempdir().unwrap();
    let mut source = init_repo(&tmp.path().join("upstream"));
    let root = tmp.path().join("follower");
    let mut repo = init_repo(&root);
    let lock = repo.git_dir.join("index.lock");
    let c1 = commit(&mut source, "first", &[("app", Some("one")), ("dir/kept", Some("stable")), ("obsolete", Some("old"))]);
    let cfg = repo.common_dir.join("config");
    config::set_in_file(&cfg, "remote.origin.url", source.workdir.to_str().unwrap()).unwrap();
    config::set_in_file(&cfg, "branch.main.merge", "refs/heads/main").unwrap();
    assert!(follow::upstream(&repo).unwrap().is_none(), "missing remote gives no upstream");
    config::set_in_file(&cfg, "branch.main.remote", ".").unwrap();
    assert!(follow::upstream(&repo).unwrap().is_none(), "a local branch remote is excluded");
    config::set_in_file(&cfg, "branch.main.remote", "origin").unwrap();
    let up = follow::upstream(&repo).unwrap().unwrap();
    assert_eq!(up.tracking_ref, "refs/remotes/origin/main", "tracking ref comes from branch config");
    let first = local_fetch(&mut repo, &up);
    assert_eq!((first.old, first.tip), (None, c1), "initial fetch finds only the upstream tip");
    repo.create_branch("main", &c1).unwrap();
    repo.checkout_branch("main").unwrap();

    let c2 = commit(&mut source, "second", &[("app", Some("two")), ("added", Some("new")), ("obsolete", None)]);
    let fetched = local_fetch(&mut repo, &up);
    assert_eq!(fetched.old, Some(c1), "fetch retains the previous tracking tip");
    assert_eq!(follow::plan(&mut repo, &up, &fetched).unwrap(), Plan::FastForward { from: c1, to: c2 }, "descendant plans a fast-forward");
    let held = follow::prepare(&mut repo, "main", c1, c2).unwrap();
    assert!(lock.is_file() && !root.join("added").exists(), "prepare holds the lock without writing files");
    let err = follow::prepare(&mut repo, "main", c1, c2).err().unwrap();
    assert!(err.to_string().contains("index.lock") && lock.is_file(), "another writer must leave the held lock alone");
    drop(held);
    assert!(!lock.exists(), "dropping a prepared update releases the lock");

    follow::prepare(&mut repo, "main", c1, c2).unwrap().apply(&mut repo).unwrap();
    assert_eq!(fs::read(root.join("app")).unwrap(), b"two", "changed file follows c2");
    assert_eq!(fs::read(root.join("dir/kept")).unwrap(), b"stable", "unchanged subtree survives");
    assert_eq!(fs::read(root.join("added")).unwrap(), b"new", "added file appears");
    assert!(!root.join("obsolete").exists(), "removed file disappears");
    assert_eq!(repo.resolve_ref("refs/heads/main").unwrap(), c2, "local branch advances");
    assert_eq!(repo.resolve_ref(&up.tracking_ref).unwrap(), c2, "tracking branch advances");
    assert_eq!(repo.head().unwrap(), RefTarget::Symbolic("refs/heads/main".into()), "HEAD stays symbolic");
    let tracked = StatusOptions { skip_untracked: true, ..Default::default() };
    assert!(repo.status_with_options(tracked).unwrap().entries.is_empty(), "index and files match c2");
    assert!(!lock.exists(), "apply publishes the held index");
    assert_eq!(follow::plan(&mut repo, &up, &fetched).unwrap(), Plan::Current, "following again is current");

    // Rewrite upstream from c1, then fetch again as a later follow round would.
    source.create_branch("rewrite", &c1).unwrap();
    source.checkout_branch("rewrite").unwrap();
    let c3 = commit(&mut source, "rewritten", &[("app", Some("three")), ("incoming", Some("upstream"))]);
    source.create_branch("main", &c3).unwrap();
    let rewrite = local_fetch(&mut repo, &up);
    assert_eq!((rewrite.old, rewrite.tip), (Some(c2), c3), "rewrite records the exact prior tracking tip");
    let rewrite = local_fetch(&mut repo, &up);
    assert_eq!(rewrite.old, Some(c3), "a second fetch already tracks the rewritten tip");
    assert_eq!(follow::plan(&mut repo, &up, &rewrite).unwrap(), Plan::Mirror { from: c2, to: c3 }, "the tracking reflog remembers the follower's old upstream tip");

    fs::write(root.join("app"), "editing").unwrap();
    assert_eq!(follow::plan(&mut repo, &up, &rewrite).unwrap(), Plan::Changes(vec!["app".into()]), "report the edited tracked path");
    let own = commit(&mut repo, "local work", &[("app", Some("editing"))]);
    assert_eq!(follow::plan(&mut repo, &up, &rewrite).unwrap(), Plan::OwnCommits, "divergence with local commits is preserved");
    assert_eq!(follow::plan(&mut repo, &up, &fetched).unwrap(), Plan::Current, "an older fetched ancestor never moves HEAD backwards");
    repo.create_branch("saved", &own).unwrap();
    repo.create_branch("main", &c2).unwrap();
    repo.checkout_branch("main").unwrap();

    fs::write(root.join("incoming"), "mine").unwrap();
    assert_eq!(follow::plan(&mut repo, &up, &rewrite).unwrap(), Plan::Mirror { from: c2, to: c3 }, "untracked files do not dirty a mirror");
    let index_before = fs::read(repo.git_dir.join("index")).unwrap();
    let refusal = follow::prepare(&mut repo, "main", c2, c3).err().unwrap();
    assert!(refusal.to_string().contains("incoming"), "prepare identifies the obstructing path");
    assert_eq!(fs::read(root.join("incoming")).unwrap(), b"mine", "refusal preserves the untracked file");
    assert_eq!(fs::read(root.join("app")).unwrap(), b"two", "refusal leaves tracked files at c2");
    assert_eq!(fs::read(repo.git_dir.join("index")).unwrap(), index_before, "refusal preserves the index");
    assert!(repo.head_oid().unwrap() == c2 && !lock.exists(), "refusal keeps HEAD and releases the lock");
    fs::remove_file(root.join("incoming")).unwrap();
    follow::prepare(&mut repo, "main", c2, c3).unwrap().apply(&mut repo).unwrap();
    assert_eq!(fs::read(root.join("app")).unwrap(), b"three", "mirror installs the rewritten content");
    assert_eq!(fs::read(root.join("incoming")).unwrap(), b"upstream", "mirror installs the formerly obstructed path");
    assert!(repo.head_oid().unwrap() == c3 && repo.status_with_options(tracked).unwrap().entries.is_empty(), "mirror lands the exact c3 tree and index");

    // The pack has just the new commit, its root tree and its changed blob.
    let c4 = commit(&mut source, "http update", &[("app", Some("four"))]);
    let tree = source.read_commit(&c4).unwrap().tree;
    let blob = source.read_tree(&tree).unwrap().entries.into_iter().find(|e| e.name == "app").unwrap().oid;
    let entries: Vec<_> = [blob, tree, c4].into_iter().map(|oid| {
        let obj = source.read_object(&oid).unwrap();
        PackEntry::Full(obj.kind, obj.data)
    }).collect();
    let (pack, _) = build_pack_bytes(&entries);
    let https = Upstream { url: "https://example.org/project".into(), fetch_url: "https://example.org/project".into(), ..up };
    let mut uploads = 0;
    let mut ads = 0;
    let mut http = |req: &makepad_git::GitHttpRequest| {
        let body = match req.method {
            GitHttpMethod::Get => {
                ads += 1;
                assert!(req.url.ends_with("/info/refs?service=git-upload-pack"), "discovery requests the upload-pack advertisement");
                let branch = format!("{c4} refs/heads/main\n");
                let head = format!("{c1} HEAD\n");
                format!("001e# service=git-upload-pack\n0000{:04x}{head}{:04x}{branch}0000", head.len()+4, branch.len()+4).into_bytes()
            }
            GitHttpMethod::Post => {
                uploads += 1;
                let negotiation = std::str::from_utf8(&req.body).unwrap();
                assert!(req.url.ends_with("/git-upload-pack"), "fetch uses the pack endpoint");
                assert!(negotiation.contains(&format!("want {c4}\n")) && negotiation.matches("want ").count() == 1, "only configured main is wanted");
                assert!(negotiation.contains(&format!("have {c3}\n")), "the follower advertises its existing tip");
                let mut body = b"0008NAK\n".to_vec();
                body.extend_from_slice(&pack);
                body
            }
        };
        Ok(GitHttpResponse { status_code: 200, headers: vec![], body })
    };
    let fetched = follow::fetch(&mut repo, &https, &mut http, &mut NoopHttpSyncHooks).unwrap();
    assert_eq!((fetched.old, fetched.tip), (Some(c3), c4), "HTTP fetch retains the prior tracking tip");
    assert_eq!(repo.resolve_ref(&https.tracking_ref).unwrap(), c4, "HTTP fetch publishes the tracking ref");
    assert_eq!(repo.read_commit(&c4).unwrap().tree, tree, "fetched tip reads from the local store");
    assert_eq!(repo.read_blob(&blob).unwrap(), b"four", "fetched blob reads from the local store");
    assert_eq!(repo.head_oid().unwrap(), c3, "fetch alone preserves local HEAD");
    follow::fetch(&mut repo, &https, &mut http, &mut NoopHttpSyncHooks).unwrap();
    assert_eq!((ads, uploads), (2, 1), "a known tip needs only ref discovery");
    let error = follow::fetch(&mut repo, &https, &mut |_| Err("timeout at https://user:secret@host/x".into()), &mut NoopHttpSyncHooks).unwrap_err();
    assert_eq!(error.to_string(), "invalid ref: upstream HTTP request failed: timeout at https://host/x", "transport diagnostics keep the reason and remove URL credentials");

    for url in ["https://EXAMPLE.org/team/apps.git/", "git@EXAMPLE.org:team/apps.git", "ssh://git@EXAMPLE.org:2222/team/apps.git"] {
        assert_eq!(follow::repo_key(url).as_deref(), Some("example.org/team/apps"), "remote forms share one public repo key");
        let FetchFrom::Https(fetch_url) = follow::fetch_from(url).unwrap() else { panic!("remote URL must select HTTP transport") };
        let expected = if url.starts_with("https:") { url } else { "https://EXAMPLE.org/team/apps.git" };
        assert_eq!(fetch_url, expected, "SSH forms fetch public HTTPS and HTTPS is preserved");
    }
    let local = tmp.path().join("project files.git");
    let path = local.to_str().unwrap().replace('\\', "/").replace(' ', "%20");
    let url = format!("file:///{}", path.trim_start_matches('/'));
    for url in [local.to_str().unwrap(), &url] {
        assert_eq!(follow::repo_key(url).as_deref(), Some("local/project files"), "a local key exposes only the leaf");
        assert!(matches!(follow::fetch_from(url), Ok(FetchFrom::Local(p)) if p == local), "file URLs and filesystem paths select the same directory");
    }
    assert!(follow::fetch_from("http://localhost:8080/repo").is_ok(), "loopback accepts HTTP with a port");
    assert!(follow::fetch_from("http://localhost.example.org/repo").is_err(), "loopback must match the whole host");
    let err = follow::fetch_from("git://token@example.org/team/repo").err().unwrap().to_string();
    assert!(err.contains("scheme git") && !err.contains("token") && !err.contains("example.org"), "unsupported scheme diagnostics omit the URL");

    // a mirror: the public URL stays the repository's key, fetches go to the longest insteadOf.
    // The mirror is bare and borrows its objects, as `git clone --bare --shared` makes one.
    let tip = source.resolve_ref("refs/heads/main").unwrap();
    let bare = tmp.path().join("mirror.git");
    fs::create_dir_all(bare.join("objects/info")).unwrap();
    fs::create_dir_all(bare.join("refs/heads")).unwrap();
    fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(bare.join("objects/info/alternates"), format!("{}\n", source.workdir.join(".git/objects").display())).unwrap();
    fs::write(bare.join("refs/heads/main"), format!("{tip}\n")).unwrap();
    config::set_in_file(&cfg, "remote.origin.url", "https://example.org/team/apps.git").unwrap();
    config::set_in_file(&cfg, "url.https://unused.example.org/.insteadOf", "https://example.org/").unwrap();
    config::set_in_file(&cfg, &format!("url.{}.insteadOf", bare.to_str().unwrap()), "https://example.org/team/apps.git").unwrap();
    let mirrored = follow::upstream(&repo).unwrap().unwrap();
    assert_eq!(follow::repo_key(&mirrored.url).as_deref(), Some("example.org/team/apps"), "a mirror keeps the public key");
    assert_eq!(mirrored.fetch_url, bare.to_str().unwrap(), "the longest insteadOf names where fetches go");
    assert_eq!(local_fetch(&mut repo, &mirrored).tip, tip, "a bare mirrored upstream fetches from the mirror, with no HTTP");
    let nested = source.workdir.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let inside = Upstream { fetch_url: nested.to_str().unwrap().into(), ..mirrored };
    let err = follow::fetch(&mut repo, &inside, &mut |_| panic!("no HTTP"), &mut NoopHttpSyncHooks).err().unwrap().to_string();
    assert!(err.contains("not a git repository"), "a local upstream never resolves to the repository around it: {err}");
}
