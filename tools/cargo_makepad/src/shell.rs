use std::{
    path::{Path, PathBuf},
    fs::File,
    io::{Write,Read},
    fs,
    process::{Command, Stdio}
};

pub fn get_crate_dir(build_crate: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().unwrap();
    if let Ok(output) = shell_env_cap(&[], &cwd, "cargo", &["pkgid", "-p", build_crate]) {
        return Ok(output.trim_start_matches("file://").split('#').next().unwrap().into())
    } else {
        Err(format!("Failed to get crate dir for: {}", build_crate))
    }
}

pub fn get_build_crate_from_args(args: &[String]) -> Result<&str, String> {
    if args.is_empty() {
        return Err("Not enough arguments to build".into());
    }
    if args[0] == "-p" {
        if args.len()<2 { 
            return Err("Not enough arguments to build".into());
        }
        Ok(&args[1])
    }
    else {
        Ok(&args[0])
    }
}

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
            Err(format!("rmdir {:?} failed {:?}", path, e))
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

pub fn cp_all(source_dir: &Path, dest_dir: &Path, exec: bool) -> Result<(), String> {
    cp_all_recursive(source_dir, dest_dir, exec)?;
    Ok(())
}

fn cp_all_recursive(source_dir: &Path, dest_dir: &Path, exec: bool) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Err(format!("{:?} is not a directory", source_dir));
    }

    mkdir(dest_dir) ?;

    for entry in fs::read_dir(source_dir).map_err(|_e| format!("Unable to read source directory {:?}", source_dir))? {
        let entry = entry.map_err(|_e| format!("Unable to process directory entry"))?;
        let source_path = entry.path();
        if source_path.is_file() {
            let dest_path = dest_dir.join(source_path.file_name()
                .ok_or_else(|| format!("Unable to get filename for {:?}", source_path))?);

            cp(&source_path, &dest_path, exec)?;
        } else if source_path.is_dir() {
            let dest_path = dest_dir.join(source_path.file_name()
                    .ok_or_else(|| format!("Unable to get folder name for {:?}", source_path))?);

            cp_all_recursive(&source_path, &dest_path, exec)?;
        }
    }

    Ok(())
}

pub fn ls(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    ls_recursive(dir, dir, &mut result)?;
    Ok(result)
}

fn ls_recursive(dir: &Path, prefix: &Path, result: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|_e| format!("Unable to read source directory {:?}", dir))? {
        let entry = entry.map_err(|_e| format!("Unable to process directory entry"))?;
        let source_path = entry.path();
        if source_path.is_file() {
            let file_name = source_path
                .file_name()
                .ok_or_else(|| format!("Unable to get filename"))?
                .to_str()
                .ok_or_else(|| format!( "Unable to convert filename to str"))?;

            if !file_name.starts_with('.') {
                let result_path = source_path.strip_prefix(prefix)
                    .map_err(|_e| format!("Unable to strip prefix"))?;
                result.push(result_path.to_path_buf());
            }
        } else if source_path.is_dir() {
            ls_recursive(&source_path, prefix, result)?;
        }
    }

    Ok(())
}