use pretty_hex::*;
use terminal_size::{Width, Height, terminal_size};

pub fn get_progressbar(length: usize, label: Option<&str>) -> indicatif::ProgressBar {
  let term_width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
      let bar_width = term_width.saturating_sub(50 + label.unwrap_or("").len()); // Account for other elements in the template
      
      let progress_bar = indicatif::ProgressBar::new(length as u64);
      progress_bar.set_style(indicatif::ProgressStyle::default_bar()
      .template(&format!("{} [{{elapsed_precise}}] [{{bar:{}.cyan/blue}}] {{bytes}}/{{total_bytes}} ({{eta}})", label.unwrap_or(""), bar_width))
      .unwrap()
      .progress_chars("#>-"));
      progress_bar
}

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