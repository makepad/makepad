use std::{
    path::{Path},
    fs::File,
    io::{Write,Read},
    fs,
    process::{Command, Stdio}
};

pub fn shell(cwd: &Path, cmd: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd_build = Command::new(cmd);
    
    cmd_build.args(args)
        .current_dir(cwd);
    
    let mut child = cmd_build.spawn().map_err( | e | format!("Error starting {} in dir {:?} - {:?}", cmd, cwd, e)) ?;
    
    let r = child.wait().map_err( | e | format!("Process {} in dir {:?} returned error {:?} ", cmd, cwd, e)) ?;
    if !r.success() {
        return Err(format!("Process {} in dir {:?} returned error exit code ", cmd, cwd));
    }
    Ok(())
}

pub fn shell_env(env: &[(&str, &str)], cwd: &Path, cmd: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd_build = Command::new(cmd);
    
    cmd_build.args(args)
        .current_dir(cwd);
        
    for (key, value) in env {
        cmd_build.env(key, value);
    }
    let mut child = cmd_build.spawn().map_err( | e | format!("Error starting {} in dir {:?} - {:?}", cmd, cwd, e)) ?;
    
    let r = child.wait().map_err( | e | format!("Process {} in dir {:?} returned error {:?} ", cmd, cwd, e)) ?;
    if !r.success() {
        return Err(format!("Process {} in dir {:?} returned error exit code ", cmd, cwd));
    }
    Ok(())
}


pub fn shell_env_cap(env: &[(&str, &str)], cwd: &Path, cmd: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd_build = Command::new(cmd);
    
    cmd_build.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(cwd);
        
    for (key, value) in env {
        cmd_build.env(key, value);
    }
    let mut child = cmd_build.spawn().map_err( | e | format!("Error starting {} in dir {:?} - {:?}", cmd, cwd, e)) ?;
    
    let r = child.wait().map_err( | e | format!("Process {} in dir {:?} returned error {:?} ", cmd, cwd, e)) ?;
    if !r.success() {
        let mut out = String::new();
        let _ = child.stderr.unwrap().read_to_string(&mut out);
        return Err(format!("Process {} in dir {:?} returned error exit code {} ", cmd, cwd, out));
    }
    let mut out = String::new();
    let _ = child.stdout.unwrap().read_to_string(&mut out);
    Ok(out)
}

pub fn write_text(path: &Path, data:&str) -> Result<(), String> {
    mkdir(path.parent().unwrap()) ?;
    match fs::File::create(path) {
        Err(e) => {
            Err(format!("file create {:?} failed {:?}", path, e))
        },
        Ok(mut f) =>{
            f.write_all(data.as_bytes())
                .map_err( | _e | format!("Cant write file {:?}", path))
        }
    }
}

pub fn rmdir(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Err(e) => {
            Err(format!("mkdir {:?} failed {:?}", path, e))
        },
        Ok(()) => Ok(())
    }
}


pub fn mkdir(path: &Path) -> Result<(), String> {
    match fs::create_dir_all(path) {
        Err(e) => {
            Err(format!("mkdir {:?} failed {:?}", path, e))
        },
        Ok(()) => Ok(())
    }
}

pub fn rm(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Err(e) => {
            Err(format!("remove_file {:?} failed {:?}", path, e))
        },
        Ok(()) => Ok(())
    }
}


#[allow(unused)]
pub fn cp(source_path: &Path, dest_path: &Path, exec: bool) -> Result<(), String> {
    let data = fs::read(source_path)
        .map_err( | _e | format!("Cant open input file {:?}", source_path)) ?;
    mkdir(dest_path.parent().unwrap()) ?;
    let mut output = File::create(dest_path)
        .map_err( | _e | format!("Cant open output file {:?}", dest_path)) ?;
    output.write(&data)
        .map_err( | _e | format!("Cant write output file {:?}", dest_path)) ?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if exec {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest_path, PermissionsExt::from_mode(0o744))
            .map_err( | _e | format!("Cant set exec permissions on output file {:?}", dest_path)) ?;
    }
    Ok(())
}

