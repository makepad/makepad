use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn current_branch(repo_root: &Path) -> String {
    let head = fs::read_to_string(repo_root.join(".git/HEAD")).unwrap();
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .unwrap_or("main")
        .to_string()
}

fn git(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

#[test]
fn test_local_clone_commit_merge() {
    let root = repo_root();
    let branch = current_branch(&root);
    let checkout1 = root.join("local/checkout1");
    let checkout2 = root.join("local/checkout2");

    // Clean previous runs
    let _ = fs::remove_dir_all(&checkout1);
    let _ = fs::remove_dir_all(&checkout2);

    println!(
        "\n=== Local depth=1 clone: {} branch={} ===",
        root.display(),
        branch
    );

    // --- Clone into checkout1 and checkout2 ---
    let t1 = makepad_git::local_clone_depth1(&root, &checkout1, Some(&branch))
        .expect("clone checkout1 failed");
    println!(
        "\ncheckout1: {:.1}ms ({} files, {:.1}MB)",
        t1.total_ms,
        t1.num_files,
        t1.bytes_written as f64 / 1_048_576.0
    );
    println!(
        "  resolve={:.1}ms setup={:.1}ms checkout={:.1}ms",
        t1.resolve_ms, t1.setup_ms, t1.checkout_ms
    );
    println!(
        "  tree_walk={:.1}ms parallel={:.1}ms",
        t1.tree_walk_ms, t1.parallel_ms
    );

    let t2 = makepad_git::local_clone_depth1(&root, &checkout2, Some(&branch))
        .expect("clone checkout2 failed");
    println!(
        "\ncheckout2: {:.1}ms ({} files, {:.1}MB)",
        t2.total_ms,
        t2.num_files,
        t2.bytes_written as f64 / 1_048_576.0
    );

    // --- Git CLI comparison ---
    let dir_c = root.join("local/checkout_gitcli");
    let _ = fs::remove_dir_all(&dir_c);
    let t_c = Instant::now();
    let _ = Command::new("git")
        .args([
            "clone",
            "--depth=1",
            "--branch",
            &branch,
            "--local",
            root.to_str().unwrap(),
            dir_c.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let ms_c = t_c.elapsed().as_secs_f64() * 1000.0;
    let _ = fs::remove_dir_all(&dir_c);
    println!("\ngit clone --depth=1: {:.1}ms", ms_c);
    println!("Speedup: {:.1}x", ms_c / t1.total_ms);

    // --- Verify checkout1 with git CLI ---
    println!("\n--- Verifying checkout1 ---");
    let (ok, stdout, stderr) = git(&checkout1, &["status", "--short"]);
    println!("  git status: ok={} entries={}", ok, stdout.lines().count());
    if !stderr.is_empty() {
        println!("  stderr: {}", stderr);
    }
    if !stdout.is_empty() {
        // Print first few entries to see what's wrong
        for line in stdout.lines().take(5) {
            println!("    {}", line);
        }
    }

    let (_, log, _) = git(&checkout1, &["log", "--oneline", "-1"]);
    println!("  git log: {}", log);

    let (_, diff, _) = git(&checkout1, &["diff", "--stat", "HEAD"]);
    if diff.is_empty() {
        println!("  working tree: clean");
    } else {
        println!("  working tree: {} files differ", diff.lines().count());
    }

    // --- Make a commit in checkout2 ---
    println!("\n--- Committing in checkout2 ---");
    fs::write(
        checkout2.join("test_from_checkout2.txt"),
        "hello from checkout2\n",
    )
    .unwrap();

    let mut repo2 = makepad_git::Repository::open(&checkout2).unwrap();
    repo2.stage_file("test_from_checkout2.txt").unwrap();
    let sig = makepad_git::Signature {
        name: "Test".into(),
        email: "info@makepad.nl".into(),
        timestamp: 1700000000,
        tz_offset: "+0000".into(),
    };
    let commit2_oid = repo2
        .commit("commit from checkout2\n", sig.clone())
        .unwrap();
    println!("  new commit: {}", commit2_oid);

    // Verify with git CLI
    let (_, log2, _) = git(&checkout2, &["log", "--oneline", "-3"]);
    println!(
        "  git log:\n{}",
        log2.lines()
            .map(|l| format!("    {}", l))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // --- Merge checkout2 into checkout1 ---
    println!("\n--- Merging checkout2 -> checkout1 ---");
    let merge_start = Instant::now();

    // Copy new objects from checkout2 to checkout1 (just the loose ones from the commit)
    let c2_objects = checkout2.join(".git/objects");
    let c1_objects = checkout1.join(".git/objects");
    copy_loose_objects(&c2_objects, &c1_objects);

    // Create a branch in checkout1 for the merge source
    let c1_git = checkout1.join(".git");
    makepad_git::refs::write_ref(&c1_git, "refs/heads/_from_checkout2", &commit2_oid).unwrap();

    let mut repo1 = makepad_git::Repository::open(&checkout1).unwrap();
    // Debug: can we read the HEAD commit through alternates?
    let head1 = repo1.head_oid().unwrap();
    println!("  checkout1 HEAD: {}", head1);
    match repo1.read_commit(&head1) {
        Ok(c) => println!("  read_commit OK: tree={}", c.tree),
        Err(e) => println!("  read_commit FAILED: {}", e),
    }
    let result = repo1.merge_branch("_from_checkout2", sig).unwrap();
    let merge_ms = merge_start.elapsed().as_secs_f64() * 1000.0;
    println!("  result: {:?}", result);
    println!("  time: {:.1}ms", merge_ms);

    // --- Verify merge result ---
    println!("\n--- Verifying merge in checkout1 ---");
    assert!(
        checkout1.join("test_from_checkout2.txt").exists(),
        "merged file must exist"
    );
    assert_eq!(
        fs::read_to_string(checkout1.join("test_from_checkout2.txt")).unwrap(),
        "hello from checkout2\n"
    );
    println!("  file content: OK");

    let (_, log1, _) = git(&checkout1, &["log", "--oneline", "-5"]);
    println!(
        "  git log:\n{}",
        log1.lines()
            .map(|l| format!("    {}", l))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let (_, head1, _) = git(&checkout1, &["rev-parse", "HEAD"]);
    println!("  HEAD: {}", head1);

    let (_ok, diff1, _) = git(&checkout1, &["diff", "--stat", "HEAD"]);
    if diff1.is_empty() {
        println!("  working tree: clean");
    } else {
        println!("  working tree: {} files differ", diff1.lines().count());
    }

    let (_, status1, _) = git(&checkout1, &["status", "--short"]);
    if status1.is_empty() {
        println!("  git status: clean");
    } else {
        println!("  git status: {} entries", status1.lines().count());
        for line in status1.lines().take(10) {
            println!("    {}", line);
        }
    }

    // --- Summary ---
    println!("\n=== SUMMARY ===");
    println!(
        "Clone:   {:.1}ms  (vs git {:.1}ms, {:.1}x faster)",
        t1.total_ms,
        ms_c,
        ms_c / t1.total_ms
    );
    println!("Merge:   {:.1}ms", merge_ms);
}

fn copy_loose_objects(src: &Path, dst: &Path) {
    if let Ok(entries) = fs::read_dir(src) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.len() == 2 && entry.path().is_dir() {
                let dst_sub = dst.join(&name);
                let _ = fs::create_dir_all(&dst_sub);
                if let Ok(sub_entries) = fs::read_dir(entry.path()) {
                    for sub in sub_entries.flatten() {
                        let dst_file = dst_sub.join(sub.file_name());
                        if !dst_file.exists() {
                            let _ = fs::copy(sub.path(), dst_file);
                        }
                    }
                }
            }
        }
    }
}
