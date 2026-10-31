// An expected unsized type is only a hint for a call's arguments
// (rustc_hir_typeck/src/expectation.rs `rvalue_hint`, fn_ctxt/checks.rs
// `expected_inputs_for_expected_output`): `Arc::new(std::mem::take(..))`
// into an `Arc<dyn Send + Sync>` slot and a `?Sized` generic fed from an
// `Arc<[u8]>` clone never require the unsized type to be Sized.
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
struct Files {
    files: Vec<u32>,
    batched: u64,
}

struct Explorer {
    retired: Vec<Arc<dyn Send + Sync>>,
    retirement_batch: Option<Arc<dyn Send + Sync>>,
    files: Files,
}

impl Explorer {
    fn flush(&mut self) {
        if self.retirement_batch.is_none() && !self.retired.is_empty() {
            self.retirement_batch = Some(Arc::new(std::mem::take(&mut self.retired)));
        }
    }

    fn plan(&mut self, individual: &mut Vec<u32>) {
        self.retired.push(Arc::new(std::mem::replace(&mut self.files.files, std::mem::take(individual))));
        self.files.batched += 1;
    }
}

struct Image {
    loaded: usize,
}

impl Image {
    fn load_from_data<D>(&mut self, _path: &Path, data: Arc<D>) -> Result<(), String>
    where
        D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
    {
        self.loaded += (*data).as_ref().len();
        Ok(())
    }
}

struct ImageRef(std::cell::RefCell<Image>);

impl ImageRef {
    fn load_from_data<D>(&self, path: &Path, data: Arc<D>) -> Result<(), String>
    where
        D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
    {
        self.0.borrow_mut().load_from_data(path, data)
    }
}

struct Thumb {
    bytes: Arc<[u8]>,
}

#[test]
fn unsized_expectations_are_hints() {
    let mut explorer = Explorer { retired: Vec::new(), retirement_batch: None, files: Files::default() };
    explorer.retired.push(Arc::new(5u8));
    explorer.flush();
    assert!(explorer.retired.is_empty());
    assert!(explorer.retirement_batch.is_some());
    explorer.files.files.push(9);
    let mut individual = vec![1, 2];
    explorer.plan(&mut individual);
    assert_eq!(explorer.files.files, vec![1, 2]);
    assert_eq!(explorer.retired.len(), 1);
    assert!(individual.is_empty());

    let image = ImageRef(std::cell::RefCell::new(Image { loaded: 0 }));
    let thumb = Thumb { bytes: Arc::from(&b"png"[..]) };
    let bytes = Some(&thumb);
    let loaded = bytes.is_some_and(|bytes| {
        let key = PathBuf::from(format!("thumb/{}", 1));
        image.load_from_data(&key, bytes.bytes.clone()).is_ok()
    });
    assert!(loaded);
    let pics: Vec<(String, Arc<[u8]>)> = vec![("a".into(), Arc::from(&b"ab"[..]))];
    match pics.get(0) {
        Some((key, png)) => {
            let _ = image.load_from_data(Path::new(key), png.clone());
        }
        None => {}
    }
    assert_eq!(image.0.borrow().loaded, 5);
}
