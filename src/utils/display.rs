use pretty_hex::*;
use terminal_size::{Width, Height, terminal_size};

pub fn hex_dump(data: Vec<u8>, offset: usize) {

    let term_width = if let Some((Width(w), Height(_h))) = terminal_size() {
        w as usize
    } else {
        0
    };

  let cfg = HexConfig {
    title: false,
    width: if term_width < 80 { 8 } else { 16 },
    group: 0,
    ascii: true,
    display_offset: offset,
    ..HexConfig::default() };

  println!("{:?}", data.hex_conf(cfg));
}