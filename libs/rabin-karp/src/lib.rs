/// Searches the given needle in the given haystack, appending the starting indices of all matches
/// to the given indices vector.
///
/// This function uses the Rabin-Karp algorithm, see:
/// https://en.wikipedia.org/wiki/Rabin%E2%80%93Karp_algorithm
///
/// for more information.
pub struct RabinKarpResult{
    pub new_line_byte: usize,
    pub line: usize,
    pub column_byte: usize,
    pub byte: usize
}

pub fn search(haystack: &[u8], needle: &[u8], indices: &mut Vec<RabinKarpResult>) {
    if needle.len()>haystack.len(){
        return
    }
    const BASE: u32 = 257;
    const MODULO: u32 = 16_711_921;

    // Compute the base to the n-th power, where n is the length of the needle.
    let mut base_pow = 1;
    for _ in 0..needle.len() - 1 {
        base_pow = (base_pow * BASE) % MODULO;
    }

    // Compute the hash of both the initial window of the haystack and the needle.
    let mut haystack_hash = 0;
    let mut needle_hash = 0;
    for index in 0..needle.len() {
        haystack_hash = (haystack_hash * BASE + haystack[index] as u32) % MODULO;
        needle_hash = (needle_hash * BASE + needle[index] as u32) % MODULO;
    }
    
    let mut line = 0;
    let mut column_byte:usize = 0;
    let mut new_line_byte = 0;
    
    for index in 0..haystack.len() - needle.len() + 1 {
        if haystack[index] == b'\n'{
            new_line_byte = index + 1;
            line += 1;
            column_byte = 0;
        }
        else{
            column_byte += 1;
        }
        // If the hash of the current window of the haystack matches the hash of the needle, we have
        // a potential match. Make sure that we have an actual match, and if so append the start
        // index of the match to the indices vector.
        if haystack_hash == needle_hash && &haystack[index..][..needle.len()] == needle {
            indices.push(RabinKarpResult{
                new_line_byte,
                line,
                column_byte: column_byte.saturating_sub(1),
                byte: index
            });
        }
        // Update the hash of the the current window of the haystack, by removing the first
        // byte from and adding the next byte to the hash.
        if index < haystack.len() - needle.len() {
            haystack_hash =
                (haystack_hash + MODULO - (haystack[index] as u32 * base_pow) % MODULO) % MODULO;
            haystack_hash = (haystack_hash * BASE + haystack[index + needle.len()] as u32) % MODULO;
        }
    }
}
