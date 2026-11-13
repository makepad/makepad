//! A program started with an argument or environment values that are not Unicode (a byte 0xff on
//! Unix, an unpaired surrogate on Windows) gets them as the OS gave them: the flight recorder read
//! them as Strings when a run started and stopped it before main (relax issue 159; this case from
//! relax PR 167). Here the program is a test's child (CARGO_BIN_EXE, a cleared environment),
//! started by the run's std with those values.
use std::ffi::OsString;
use std::process::Command;

#[cfg(unix)]
fn not_unicode(bytes: &[u8]) -> OsString {
    std::os::unix::ffi::OsStringExt::from_vec(bytes.to_vec())
}

#[cfg(windows)]
fn not_unicode(bytes: &[u8]) -> OsString {
    let wide: Vec<u16> = bytes.iter().map(|&b| if b < 0x80 { b as u16 } else { 0xd800 }).collect();
    std::os::windows::ffi::OsStringExt::from_wide(&wide)
}

fn stdout_of(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "the program ends: {:?}\n{stdout}\n{stderr}", out.status);
    stdout
}

#[test]
fn launch_values_need_not_be_unicode() {
    let exe = env!("CARGO_BIN_EXE_non-unicode-launch");
    assert_eq!(stdout_of(Command::new(exe).env_clear()), "None None None\n", "a start with no argument in a cleared environment");
    let (arg, value, recorded) = (not_unicode(b"\xff"), not_unicode(b"a\xfeb"), not_unicode(b"\xfe"));
    let got = stdout_of(Command::new(exe).env_clear().arg(&arg).env("NON_UNICODE_VALUE", &value).env("MAKEPAD_NON_UNICODE", &recorded));
    assert_eq!(got, format!("{:?} {:?} {:?}\n", Some(&arg), Some(&value), Some(&recorded)), "the argument and the values as they were given");
}
