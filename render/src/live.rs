use crate::cx::*;

#[derive(PartialEq, Copy, Clone, Hash, Eq)]
pub struct LiveId(pub u64);

pub const fn str_to_liveid(modstr: &str, idstr: &str) -> LiveId {
    let modpath = modstr.as_bytes();
    let modpath_len = modpath.len();
    let id = idstr.as_bytes();
    let id_len = id.len();
    
    let mut value = 0u64;
    if id.len()>5
        && id[0] == 's' as u8
        && id[1] == 'e' as u8
        && id[2] == 'l' as u8
        && id[3] == 'f' as u8
        && id[4] == ':' as u8 {
        
        let mut o = 0;
        let mut i = 0;
        while i < modpath_len {
            value ^= (modpath[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        let mut i = 4;
        while i < id_len {
            value ^= (id[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        return LiveId(value)
    }
    if id.len()>6
        && id[0] == 'c' as u8
        && id[1] == 'r' as u8
        && id[2] == 'a' as u8
        && id[3] == 't' as u8
        && id[4] == 'e' as u8
        && id[5] == ':' as u8 {
        let mut o = 0;
        let mut i = 0;
        while i < modpath_len {
            if modpath[i] == ':' as u8{
                break
            }
            value ^= (modpath[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        let mut i = 5;
        while i < id_len {
            value ^= (id[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        return LiveId(value)
    }
    let mut i = 0;
    let mut o = 0;
    while i < id_len {
        value ^= (id[i] as u64) << ((o & 7) << 3);
        o += 1;
        i += 1;
    }
    LiveId(value)
}

impl Cx{
    pub fn get_color(&mut self,  id: LiveId)->Color{
        for style_id in &self.live_stack {
            if let Some(value) = self.lives[*style_id].colors.get(&id) {
                return *value
            }
        }
        *self.live_base.colors.get(&id).expect("Cannot find color")
    }
}


#[macro_export]
macro_rules!color {
    ( $ cx: ident, $id: ident) => {
        $cx.get_color(str_to_liveid(module_path!(), $id))
    }
}
