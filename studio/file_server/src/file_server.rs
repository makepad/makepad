use {
    makepad_shell::*,
    crate::{
        makepad_file_protocol::*,
    },
    std::{
        time::Instant,
        thread,
        cmp::Ordering,
        fmt,
        fs,
        str,
        time::Duration,
        path::{Path, PathBuf},
        sync::{Arc, RwLock, Mutex},
    },
};

pub struct FileServer {
    // The id for the next connection
    next_connection_id: usize,
    // State that is shared between every connection
    shared: Arc<RwLock<Shared >>,
}

impl FileServer {
    /// Creates a new collab server rooted at the given path.
    pub fn new(roots: FileSystemRoots) -> FileServer {
        FileServer {
            next_connection_id: 0,
            shared: Arc::new(RwLock::new(Shared {
                roots,
            })),
        }
    }
    
    /// Creates a new connection to this collab server, and returns a handle for the connection.
    ///
    /// The given `notification_sender` is called whenever the server wants to send a notification
    /// for this connection. The embedder is responsible for sending the notification.
    pub fn connect(&mut self, notification_sender: Box<dyn NotificationSender>) -> FileServerConnection {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        FileServerConnection {
            _connection_id:connection_id,
            shared: self.shared.clone(),
            _notification_sender: notification_sender,
            open_files: Default::default(),
            stop_observation: Default::default()
        }
    }
}

/// A connection to a collab server.
pub struct FileServerConnection {
    // The id for this connection.
    _connection_id: ConnectionId,
    // State is shared between every connection.
    shared: Arc<RwLock<Shared >>,
    // Used to send notifications for this connection.
    _notification_sender: Box<dyn NotificationSender>,
    open_files: Arc<Mutex<Vec<(String, u64, Vec<u8>)>>>,
    
    stop_observation: Arc<Mutex<bool>>,
}

impl FileServerConnection {
    /// Handles the given `request` for this connection, and returns the corresponding response.
    ///
    /// The embedder is responsible for receiving requests, calling this method to handle them, and
    /// sending back the response.
    pub fn handle_request(&self, request: FileRequest) -> FileResponse {
        match request {
            FileRequest::Search{set, id}=>{
                self.search_start(set, id);
                FileResponse::SearchInProgress(id)
            }
            FileRequest::LoadFileTree {with_data} => FileResponse::LoadFileTree(self.load_file_tree(with_data)),
            FileRequest::OpenFile{path,id} => FileResponse::OpenFile(self.open_file(path, id)),
            FileRequest::SaveFile{path, data, id, patch} => FileResponse::SaveFile(self.save_file(path, data, id, patch)),
            FileRequest::LoadSnapshotImage{root, hash}=>FileResponse::LoadSnapshotImage(self.load_snapshot_image(root, hash)),
            FileRequest::SaveSnapshotImage{root, hash, data}=>FileResponse::SaveSnapshotImage(self.save_snapshot_image(root, hash, data)),
            FileRequest::CreateSnapshot{root, message}=>FileResponse::CreateSnapshot(self.create_snapshot(root, message)),
            FileRequest::LoadSnapshot{root, hash}=>FileResponse::LoadSnapshot(self.load_snapshot(root, hash)),
                                    
        }
    }
    
    fn search_start(&self, what:Vec<SearchItem>, id:u64) {
        let mut sender = self._notification_sender.clone();
        let roots = self.shared.read().unwrap().roots.clone();
        thread::spawn(move || {
            
            // A recursive helper function for traversing the entries of a directory and creating the
            // data structures that describe them.
            fn search_files(id: u64, set:&Vec<SearchItem>, path: &Path, string_path:&str, sender: &mut Box<dyn NotificationSender>, last_send: &mut Instant, results: &mut Vec<SearchResult>) {
                if let Ok(entries) = fs::read_dir(path){
                    for entry in entries{
                        if let Ok(entry) = entry{
                            let entry_path = entry.path();
                            let name = entry.file_name();
                            if let Ok(name) = name.into_string() {
                                if entry_path.is_file() && !name.ends_with(".rs") || entry_path.is_dir() && name == "target"
                                || name.starts_with('.') {
                                    continue;
                                }
                            }
                            else {
                                // Skip over entries with a non UTF-8 file name.
                                continue;
                            }
                            
                            let entry_string_name = entry.file_name().to_string_lossy().to_string();
                            let entry_string_path = if string_path != ""{
                                format!("{}/{}", string_path, entry_string_name)
                            }else {
                                entry_string_name
                            };
                            
                            if entry_path.is_dir() {
                                search_files(id, set, &entry_path, &entry_string_path, sender, last_send, results);
                            }
                            else if entry_path.is_file() {
                                let mut rk_results = Vec::new();
                                if let Ok(bytes) = fs::read(&entry_path){
                                    // lets look for what in bytes
                                    // if we find thigns we emit it on the notification send
                                    fn is_word_char(b: u8)->bool{
                                        b == b'_' || b == b':' || b >= b'0' && b<= b'9' || b >= b'A' && b <= b'Z' || b >= b'a' && b <= b'z' || b>126
                                    }
                                    for item in set{
                                        let needle_bytes = item.needle.as_bytes();
                                        if needle_bytes.len()==0{
                                            continue;
                                        }
                                        makepad_rabin_karp::search(&bytes, &needle_bytes, &mut rk_results);
                                        for result in &rk_results{
                                            
                                            if item.pre_word_boundary && result.byte > 0 && is_word_char(bytes[result.byte-1]){
                                                continue
                                            }
                                            if item.post_word_boundary && result.byte + needle_bytes.len() < bytes.len() && is_word_char(bytes[result.byte + needle_bytes.len()]){
                                                continue
                                            }
                                            if let Some(prefixes) = &item.prefixes{
                                                // alright so prefixes as_bytes should be right before the match
                                                if !prefixes.iter().any(|prefix|{
                                                    let pb = prefix.as_bytes();
                                                    if result.byte > pb.len(){
                                                        if &bytes[result.byte - pb.len()..result.byte] == pb{
                                                            return true
                                                        }
                                                    }
                                                    false
                                                }){
                                                    continue
                                                }
                                            }
                                             
                                            let mut line_count = 0;
                                            for i in result.new_line_byte..bytes.len()+1{
                                                if i < bytes.len() && bytes[i] == b'\n'{
                                                    line_count += 1;
                                                }
                                                if i == bytes.len() || bytes[i] == b'\n' && line_count == 4{
                                                    if let Ok(result_line) = str::from_utf8(&bytes[result.new_line_byte..i]){
                                                        // lets output it to our results
                                                       results.push(SearchResult{
                                                            file_name: entry_string_path.clone(),
                                                            line: result.line,
                                                            column_byte: result.column_byte,
                                                            result_line: result_line.to_string()
                                                        });
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                        rk_results.clear();
                                    }
                                }
                            }
                        }
                        // lets compare time
                        if last_send.elapsed().as_secs_f64()>0.1{
                            *last_send = Instant::now();
                            let  mut new_results = Vec::new();
                            std::mem::swap(results, &mut new_results);
                            sender.send_notification(FileNotification::SearchResults{
                                id,
                                results: new_results
                            });
                            
                        }
                    }
                }
            }
            let mut last_send = Instant::now();
            let mut results = Vec::new();
            for (root_name, root_path) in roots.roots{
                search_files(id, &what, &root_path, &root_name, &mut sender, &mut last_send, &mut results);
            }
            if results.len()>0{
                sender.send_notification(FileNotification::SearchResults{
                    id,
                    results
                });
            }
        });
    }
    
    fn create_snapshot(&self, root:String, message:String) -> Result<CreateSnapshotResponse, CreateSnapshotError> {
        let root_path = self.shared.read().unwrap().roots.find_root(&root).map_err(|error|{
            CreateSnapshotError{error:format!("{:?}",error), root:root.clone()}
        })?;
        
        match shell_env_cap(&[], &root_path, "git", &["commit", "-a",&format!("-m {message}")]) {
            Ok(_) => {
                match shell_env_cap(&[], &root_path, "git", &["log", "--pretty=format:%H","--max-count=1"]) {
                    Ok(stdout) => {
                        // ok we have the last commit hash, return that
                        Ok(CreateSnapshotResponse{
                            root,
                            hash: stdout.trim().to_string()
                        })
                    }
                    // we expect it on stderr
                    Err(e) => {
                        Err(CreateSnapshotError{root, error:e})
                    }
                }
            }
            // we expect it on stderr
            Err(e) => {
                Err(CreateSnapshotError{root, error:e})
            }
        }
    }
    
        
    fn load_snapshot(&self, root:String, hash:String) -> Result<LoadSnapshotResponse, LoadSnapshotError> {
        
        let root_path = self.shared.read().unwrap().roots.find_root(&root).map_err(|error|{
            LoadSnapshotError{error:format!("{:?}",error), root:root.clone()}
        })?;
                
        match shell_env_cap(&[], &root_path, "git", &["checkout", &hash]) {
            Ok(_) => {
                Ok(LoadSnapshotResponse{root, hash})
            }
            // we expect it on stderr
            Err(e) => {
                Err(LoadSnapshotError{root, error:e})
            }
        }
    }

    // Handles a `LoadFileTree` request.
    fn load_file_tree(&self, with_data: bool) -> Result<FileTreeData, FileError> {
        // A recursive helper function for traversing the entries of a directory and creating the
        // data structures that describe them.
        fn get_directory_entries(path: &Path, with_data: bool) -> Result<Vec<DirectoryEntry>, FileError> {
            let mut entries = Vec::new();
            for entry in fs::read_dir(path).map_err( | error | FileError::Unknown(error.to_string())) ? {
                // We can't get the entry for some unknown reason. Raise an error.
                let entry = entry.map_err( | error | FileError::Unknown(error.to_string())) ?;
                // Get the path for the entry.
                let entry_path = entry.path();
                // Get the file name for the entry.
                let name = entry.file_name();
                if let Ok(name_string) = name.into_string() {
                    if entry_path.is_dir() && name_string == "target"
                        || name_string.starts_with('.') {
                        // Skip over directories called "target". This is sort of a hack. The reason
                        // it's here is that the "target" directory for Rust projects is huge, and
                        // our current implementation of the file tree widget is not yet fast enough
                        // to display vast numbers of nodes. We paper over this by pretending the
                        // "target" directory does not exist.
                        continue;
                    }
                }
                else {
                    // Skip over entries with a non UTF-8 file name.
                    continue;
                }
                // Create a `DirectoryEntry` for this entry and add it to the list of entries.
                entries.push(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().to_string(),
                    node: if entry_path.is_dir() {
                        // If this entry is a subdirectory, recursively create `DirectoryEntry`'s
                        // for its entries as well.
                        FileNodeData::Directory {
                            git_log: None,
                            entries: get_directory_entries(&entry_path, with_data) ?,
                        }
                    } else if entry_path.is_file() {
                        if with_data {
                            let bytes: Vec<u8> = fs::read(&entry_path).map_err(
                                | error | FileError::Unknown(error.to_string())
                            ) ?;
                            FileNodeData::File {data: Some(bytes)}
                        }
                        else {
                            FileNodeData::File {data: None}
                        }
                    }
                    else {
                        // If this entry is neither a directory or a file, skip it. This ignores
                        // things such as symlinks, for which we are not yet sure how we want to
                        // handle them.
                        continue
                    },
                });
            }
            
            // Sort all the entries by name, directories first, and files second.
            entries.sort_by( | entry_0, entry_1 | {
                match &entry_0.node {
                    FileNodeData::Directory {..} => match &entry_1.node {
                        FileNodeData::Directory {..} => entry_0.name.cmp(&entry_1.name),
                        FileNodeData::File {..} => Ordering::Less
                    }
                    FileNodeData::File {..} => match &entry_1.node {
                        FileNodeData::Directory {..} => Ordering::Greater,
                        FileNodeData::File {..} => entry_0.name.cmp(&entry_1.name)
                    }
                }
            });
            Ok(entries)
        }
        
        let roots = self.shared.read().unwrap().roots.clone();
        let mut entries = Vec::new();
        for (root_name, root_path) in roots.roots{
            let mut commits = Vec::new();
            match shell_env_cap(&[], &root_path, "git", &["log", "--pretty=format:%H %s"]) {
                Ok(stdout) => {
                    for line in stdout.split("\n"){
                        let mut parts = line.splitn(2," ");
                        if let Some(hash) = parts.next(){
                            if let Some(message) = parts.next(){
                                if hash.len() == 40{
                                    // we have something
                                    commits.push(GitCommit{
                                        hash: hash.to_string(),
                                        message: message.to_string()
                                    })
                                }
                            }
                        }
                    }
                }
                // we expect it on stderr
                Err(_e) => {}
            }
            
            entries.push(DirectoryEntry{
                name: root_name.clone(),
                node: FileNodeData::Directory {
                    git_log: Some(GitLog{
                        root: root_name,
                        commits
                    }),
                    entries: get_directory_entries(&root_path, with_data) ?,
                }
            });
        }
        Ok(FileTreeData {root_path: "".into(), root:FileNodeData::Directory {
            git_log: None,
            entries,
        }})
    }
    
    
    fn start_observation(&self) {
        let open_files = self.open_files.clone();
        let shared = self.shared.clone();
        let notification_sender = self._notification_sender.clone();
        let stop_observation = self.stop_observation.clone();
        thread::spawn(move || {
            while !*stop_observation.lock().unwrap(){
                if let Ok(mut files) = open_files.lock(){
                    for (path, file_id, last_content) in files.iter_mut() {
                        let full_path = {
                            shared.read().unwrap().roots.make_full_path(&path)
                        }.unwrap();
                        if let Ok(bytes) = fs::read(&full_path) {
                            if bytes.len() > 0 && bytes != *last_content {
                                let new_data = String::from_utf8_lossy(&bytes);
                                let old_data = String::from_utf8_lossy(&last_content);
                                // Send notification of external file change.
                                notification_sender
                                .send_notification(FileNotification::FileChangedOnDisk(
                                    SaveFileResponse{
                                        path: path.to_string(),
                                        new_data: new_data.to_string(),
                                        old_data: old_data.to_string(),
                                        kind: SaveKind::Observation,
                                        id: *file_id
                                    }
                                ));
                                *last_content = bytes;
                            }
                        }
                    }
                }
                // Sleep for 500ms.
                thread::sleep(Duration::from_millis(100));
            }
        });
    }
    
    fn load_snapshot_image(&self, root: String, hash:String) -> Result<LoadSnapshotImageResponse, LoadSnapshotImageError> {
        // alright letrs find the root
        let root_path = self.shared.read().unwrap().roots.find_root(&root).map_err(|error|{
            LoadSnapshotImageError{error, root:root.clone(), hash:hash.clone()}
        })?;
        let path = root_path.join("snapshots").join(&hash).with_extension("jpg");
        let bytes = fs::read(&path).map_err(
            | error | LoadSnapshotImageError{error:FileError::Unknown(error.to_string()), root:root.clone(), hash:hash.clone()}
        ) ?;
        
        return Ok(LoadSnapshotImageResponse{
            root,
            hash,
            data: bytes,
        })
    }
    
    fn save_snapshot_image(&self, root: String, hash:String, data:Vec<u8>) -> Result<SaveSnapshotImageResponse, FileError> {
        // alright letrs find the root
        let root_path = self.shared.read().unwrap().roots.find_root(&root)?;
        let path = root_path.join("snapshots").join(&hash).with_extension("jpg");
                
        fs::write(&path, data).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?;
                
        return Ok(SaveSnapshotImageResponse{
            root,
            hash,
        })
    }
    
    // Handles an `OpenFile` request.
    fn open_file(&self, child_path: String, id:u64) -> Result<OpenFileResponse, FileError> {
        let path = self.shared.read().unwrap().roots.make_full_path(&child_path)?;
        
        let bytes = fs::read(&path).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?;
        
        let mut open_files = self.open_files.lock().unwrap();
        
        if open_files.iter().find(|(cp,_,_)| *cp == child_path).is_none(){
            open_files.push((child_path.clone(), id, bytes.clone()));
        }
        
        if open_files.len() == 1 {
            self.start_observation();
        }
        // Converts the file contents to a `Text`. This is necessarily a lossy conversion
        // because `Text` assumes everything is UTF-8 encoded, and this isn't always the
        // case for files on disk (is this a problem?)
        /*let text: Text = Text::from_lines(String::from_utf8_lossy(&bytes)
            .lines()
            .map( | line | line.chars().collect::<Vec<_ >> ())
            .collect::<Vec<_ >>());*/
        
        let text = String::from_utf8_lossy(&bytes);
        Ok(OpenFileResponse{
            path: child_path,
            data: text.to_string(),
            id
        })
    }
    
    // Handles an `ApplyDelta` request.
    fn save_file(
        &self,
        child_path: String,
        new_data: String,
        id: u64,
        patch: bool
    ) -> Result<SaveFileResponse, FileError> {
        let mut open_files = self.open_files.lock().unwrap();
                
        if let Some(of) = open_files.iter_mut().find(|(cp,_,_)| *cp == child_path){
            of.2 =  new_data.as_bytes().to_vec();
        }
        else{
            open_files.push((child_path.clone(), id, new_data.as_bytes().to_vec()));
        }
        
        let path = self.shared.read().unwrap().roots.make_full_path(&child_path)?;
        
        let old_data = String::from_utf8_lossy(&fs::read(&path).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?).to_string();

        fs::write(&path, &new_data).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?;
        
        Ok(SaveFileResponse{
            path: child_path, 
            old_data,
            new_data,
            id,
            kind: if patch{SaveKind::Patch}else{SaveKind::Save}
        })
    }
}

/// A trait for sending notifications over a connection.
pub trait NotificationSender: Send {
    /// This method is necessary to create clones of boxed trait objects.
    fn box_clone(&self) -> Box<dyn NotificationSender>;
    
    /// This method is called to send a notification over the corresponding connection.
    fn send_notification(&self, notification: FileNotification);
}

impl<F: Clone + Fn(FileNotification) + Send + 'static> NotificationSender for F {
    fn box_clone(&self) -> Box<dyn NotificationSender> {
        Box::new(self.clone())
    }
    
    fn send_notification(&self, notification: FileNotification) {
        self (notification)
    }
}

impl Clone for Box<dyn NotificationSender> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

impl fmt::Debug for dyn NotificationSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NotificationSender")
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileSystemRoots{
    pub roots: Vec<(String, PathBuf)>
}

impl FileSystemRoots{
    pub fn map_path(&self, possible_root:&str, what:&str)->String{

        let what_path = Path::new(what);
        if what_path.is_absolute(){
            for (root_name, root_path) in &self.roots{
                if let Ok(end) = what_path.strip_prefix(root_path){
                    if let Ok(end) = end.to_path_buf().into_os_string().into_string(){
                        return format!("{root_name}/{end}");
                    }
                }
            }
            return what.to_string()
        }
        else{
            if possible_root.len() == 0{
                what.to_string()
            }
            else{
                format!("{possible_root}/{}",  what)
            }
        }
    }
    
    pub fn find_root(&self, root:&str)->Result<PathBuf,FileError>{
        if let Some(p) = self.roots.iter().find(|v| v.0 == root){
            Ok(p.1.clone())
        }
        else{
            Err(FileError::RootNotFound(root.to_string()))
        }
    }
    
    fn make_full_path(&self, child_path:&String)->Result<PathBuf,FileError>{
        let mut parts = child_path.splitn(2,"/");
        let root = parts.next().unwrap();
        let file = parts.next().unwrap();
        for (root_name, root_path) in &self.roots{
            // lets split off the first directory
            if root_name == root{
                let mut path = root_path.clone();
                path.push(file);
                return Ok(path)
            }
        }
        return Err(FileError::RootNotFound(child_path.clone()))
    }
}

// State that is shared between every connection.
#[derive(Debug)]
struct Shared {
    roots: FileSystemRoots
}

impl Shared{
        
}

/// An identifier for a connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);

