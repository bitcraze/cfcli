use crazyflie_lib::Value;
use pretty_hex::*;
use std::io::IsTerminal;
use terminal_size::{Width, Height, terminal_size};

/// Plain numeric form of a `Value` for CSV/script consumers. The lib's
/// `Debug` impl wraps the number in the type name (`U8(42)`) which is fine
/// for humans but unhelpful when piping into another tool.
pub fn value_to_csv_string(v: &Value) -> String {
    match v {
        Value::U8(x) => x.to_string(),
        Value::U16(x) => x.to_string(),
        Value::U32(x) => x.to_string(),
        Value::U64(x) => x.to_string(),
        Value::I8(x) => x.to_string(),
        Value::I16(x) => x.to_string(),
        Value::I32(x) => x.to_string(),
        Value::I64(x) => x.to_string(),
        Value::F16(x) => x.to_string(),
        Value::F32(x) => x.to_string(),
        Value::F64(x) => x.to_string(),
    }
}

/// Quote a single CSV field per RFC 4180: wrap in `"` if it contains a
/// comma, quote, CR, or LF, and double up any embedded quotes.
pub fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        field.to_string()
    }
}

/// Print a single CSV row from the given fields, escaping each field as needed.
pub fn csv_row(fields: &[&str]) {
    let row: Vec<String> = fields.iter().map(|f| csv_escape(f)).collect();
    println!("{}", row.join(","));
}

/// Finish an `indicatif::ProgressBar` with a completion message that is
/// visible in both TTY and non-TTY contexts. In a TTY the bar already
/// displays the message as its final state; in a non-TTY (pipe, subshell,
/// captured output) the bar is hidden, so we also emit the message via a
/// plain `println!` so consumers actually see that the command succeeded.
pub fn finish_progress(bar: &indicatif::ProgressBar, message: impl Into<String>) {
    let msg = message.into();
    if !std::io::stderr().is_terminal() {
        println!("{}", msg);
    }
    bar.finish_with_message(msg);
}

pub fn get_progressbar(length: usize, label: Option<&str>) -> indicatif::ProgressBar {
  use std::fmt::Write;
  let term_width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
      let bar_width = term_width.saturating_sub(50 + label.unwrap_or("").len()); // Account for other elements in the template

      let progress_bar = indicatif::ProgressBar::new(length as u64);
      progress_bar.set_style(indicatif::ProgressStyle::default_bar()
      .template(&format!("{} [{{elapsed_precise}}] [{{bar:{}.cyan/blue}}] {{bytes}}/{{total_bytes}} ({{eta}})", label.unwrap_or(""), bar_width))
      .unwrap()
      // Once the bar is finished, "eta 0s" is useless — swap it for the
      // average transfer rate over the whole operation.
      .with_key("eta", |state: &indicatif::ProgressState, w: &mut dyn Write| {
          if state.is_finished() {
              let _ = write!(w, "{}/s", indicatif::BinaryBytes(state.per_sec() as u64));
          } else {
              let _ = write!(w, "{:#}", indicatif::HumanDuration(state.eta()));
          }
      })
      .progress_chars("#>-"));
      if !std::io::stderr().is_terminal() {
          progress_bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());
      }
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