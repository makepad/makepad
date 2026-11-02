//! RELAX_CLI_EXIT_BOOT_FAIL=1: the build script fails, so a boot of this program fails after its
//! run window opened (cli_exit.sh: a hidden live run then ends with a failure code instead of
//! waiting for a retry nobody can give).
fn main() {
    println!("cargo:rerun-if-env-changed=RELAX_CLI_EXIT_BOOT_FAIL");
    if std::env::var_os("RELAX_CLI_EXIT_BOOT_FAIL").is_some() {
        eprintln!("relax-cli-exit: RELAX_CLI_EXIT_BOOT_FAIL is set: the build script fails");
        std::process::exit(1);
    }
}
