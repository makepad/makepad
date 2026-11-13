int snd_rust_side(int x);
int snd_c_entry(int x) { return snd_rust_side(x) + 1; }
