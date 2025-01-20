use {
    crate::font_atlas::CxFont,
    makepad_platform::*,
    std::{collections::HashMap, iter::{Enumerate, IntoIterator}, ops::{Index, IndexMut}, rc::Rc, slice},
};

/// A font loader.
#[derive(Debug)]
pub struct FontLoader {
    pub fonts: Vec<Option<CxFont>>,
    pub ids_by_path: HashMap<Rc<str>, usize>,
    pub paths_by_id: HashMap<usize, Rc<str>>,
}

impl FontLoader {
    /// Creates a new, empty font loader.
    pub fn new() -> Self {
        Self {
            fonts: Vec::new(),
            ids_by_path: HashMap::new(),
            paths_by_id: HashMap::new(),
        }
    }

    /// Returns true if the loader has loaded a font with the given id.
    pub fn loaded_id(&self, id: usize) -> bool {
        self.paths_by_id.contains_key(&id)
    }

    /// Returns true if the loader has loaded font with the given path.
    pub fn loaded_path(&self, path: &str) -> bool {
        self.ids_by_path.contains_key(path)
    }

    /// Returns the id of the font with the given path, if it has been loaded.
    pub fn id(&self, path: &str) -> Option<usize> {
        self.ids_by_path.get(path).copied()
    }

    /// Returns the path of the font with the given id, if it has been loaded.
    pub fn path(&self, id: usize) -> Option<&Rc<str>> {
        self.paths_by_id.get(&id)
    }

    /// Returns a reference to the font with the given id, if it has been loaded.
    pub fn get(&self, id: usize) -> Option<&Option<CxFont>> {
        self.fonts.get(id)
    }

    /// Returns an iterator over references to the loaded fonts.
    /// 
    /// Yields tuples of:
    /// - The id of the font.
    /// - The path of the font.
    /// - A reference to the font.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.fonts.iter().enumerate(),
            paths_by_id: &self.paths_by_id,
        }
    }

    /// Returns a mutable reference to the font with the given id, if it has been loaded.
    pub fn get_mut(&mut self, id: usize) -> Option<&mut Option<CxFont>> {
        self.fonts.get_mut(id)
    }

    /// Returns an iterator over mutable references to the loaded fonts.
    /// 
    /// Yields tuples of:
    /// - The id of the font.
    /// - The path of the font.
    /// - A mutable reference to the font.
    pub fn iter_mut(&mut self) -> IterMut<'_> {
        IterMut {
            iter: self.fonts.iter_mut().enumerate(),
            paths_by_id: &self.paths_by_id,
        }
    }

    /// Loads a font at the given path and returns its id.
    pub fn load(&mut self, cx: &mut Cx, path: &str) -> usize {
        let id = self.fonts.len();
        self.fonts.push(load(cx, path));
        let path: Rc<str> = path.into();
        self.paths_by_id.insert(id, path.clone());
        self.ids_by_path.insert(path.clone(), id);
        id
    }

    /// Returns the id of the font with the given path, or loads it if it doesn't exist.
    pub fn get_or_load(&mut self, cx: &mut Cx, path: &str) -> usize {
        if !self.loaded_path(path) {
            self.load(cx, path);
        }
        self.id(path).unwrap()
    }
}

impl Index<usize> for FontLoader {
    type Output = Option<CxFont>;

    fn index(&self, id: usize) -> &Self::Output {
        self.get(id).unwrap()
    }
}

impl IndexMut<usize> for FontLoader {
    fn index_mut(&mut self, id: usize) -> &mut Self::Output {
        self.get_mut(id).unwrap()
    }
}

impl<'a> IntoIterator for &'a FontLoader {
    type Item = (usize, &'a str, &'a Option<CxFont>);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut FontLoader {
    type Item = (usize, &'a str, &'a mut Option<CxFont>);
    type IntoIter = IterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// An iterator over references to the loaded fonts.
/// 
/// This struct is created by the `iter` method on `FontLoader`. See its documentation for more.
#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: Enumerate<slice::Iter<'a, Option<CxFont>>>,
    paths_by_id: &'a HashMap<usize, Rc<str>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (usize, &'a str, &'a Option<CxFont>);

    fn next(&mut self) -> Option<Self::Item> {
        let (id, font) = self.iter.next()?;
        let path = &self.paths_by_id[&id];
        Some((id, path, font))
    }
}

/// A mutable iterator over references to the loaded fonts.
/// 
/// This struct is created by the `iter_mut` method on `FontLoader`. See its documentation for more.
#[derive(Debug)]
pub struct IterMut<'a> {
    iter: Enumerate<slice::IterMut<'a, Option<CxFont>>>,
    paths_by_id: &'a HashMap<usize, Rc<str>>,
}

impl<'a> Iterator for IterMut<'a> {
    type Item = (usize, &'a str, &'a mut Option<CxFont>);

    fn next(&mut self) -> Option<Self::Item> {
        let (id, font) = self.iter.next()?;
        let path = &self.paths_by_id[&id];
        Some((id, path, font))
    }
}

fn load(cx: &mut Cx, path: &str) -> Option<CxFont> {
    match cx.take_dependency(&path) {
        Ok(data) => match CxFont::load_from_ttf_bytes(data) {
            Ok(font) => Some(font),
            Err(_) => {
                error!("failed to parse font at path {:?}", path);
                None
            }
        },
        Err(_) => {
            error!("failed to load font at path {:?}", path);
            None
        }
    }
}
