use crazyflie_lib::Value;
use tabled::settings::{object::Columns, style::HorizontalLine, Modify, Padding, Style};
use tabled::{Table, Tabled};
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
/// Build a table in the style the CLI uses everywhere: columns separated by
/// ` | ` with a dashed rule under the header, sized to fit their content.
///
/// Returned rather than printed so a caller can adjust it first (for example
/// right-aligning a numeric column) before handing it to [`print_table`].
pub fn table<I>(rows: I) -> Table
where
    I: IntoIterator,
    I::Item: Tabled,
{
    let mut table = Table::new(rows);
    style(&mut table);
    table
}

/// A table whose columns are only known at run time (`header`), in the same
/// style as [`table`].
pub fn table_from_records(header: &[String], rows: &[Vec<String>]) -> Table {
    let mut builder = tabled::builder::Builder::default();
    builder.push_record(header);
    for row in rows {
        builder.push_record(row);
    }
    let mut table = builder.build();
    style(&mut table);
    table
}

fn style(table: &mut Table) {
    table
        .with(
            Style::empty()
                .vertical('|')
                .horizontals([(1, HorizontalLine::new('-').intersection('+'))]),
        )
        // The first column sits flush left; every other column keeps the
        // single space that separates it from the `|`.
        .with(Modify::new(Columns::first()).with(Padding::new(0, 1, 0, 0)));
}

/// Print a table built by [`table`], without trailing whitespace on any row.
pub fn print_table(table: &Table) {
    for line in table.to_string().lines() {
        println!("{}", line.trim_end());
    }
}

/// A column of a [`StreamTable`].
pub struct Column<'a> {
    pub name: &'a str,
    /// The width to start with; the header widens it if it is longer.
    pub width: usize,
    /// Right-aligned, for numbers.
    pub right: bool,
}

/// A table printed a row at a time, for output that streams, in the style
/// of [`table`]. The rows aren't known up front, so each column starts as
/// wide as its header or its given width, whichever is wider. A value that
/// doesn't fit widens its column from then on: one row shifts, the ones
/// after line up again.
pub struct StreamTable {
    widths: Vec<usize>,
    right: Vec<bool>,
}

impl StreamTable {
    /// Print the header of a table with these columns.
    pub fn new(columns: &[Column]) -> Self {
        let table = StreamTable {
            widths: columns.iter().map(|c| c.name.len().max(c.width)).collect(),
            right: columns.iter().map(|c| c.right).collect(),
        };
        let names: Vec<&str> = columns.iter().map(|c| c.name).collect();
        println!("{}", table.line(&names));
        println!("{}", table.rule());
        table
    }

    /// Print one row, flushed so that a consumer reading a pipe sees it
    /// right away.
    pub fn row(&mut self, fields: &[&str]) {
        use std::io::Write;
        for (width, field) in self.widths.iter_mut().zip(fields) {
            *width = (*width).max(field.len());
        }
        println!("{}", self.line(fields));
        let _ = std::io::stdout().flush();
    }

    fn line(&self, fields: &[&str]) -> String {
        let mut line = String::new();
        for (i, (field, width)) in fields.iter().zip(&self.widths).enumerate() {
            if i > 0 {
                line.push_str("| ");
            }
            if self.right[i] {
                line.push_str(&format!("{:>width$} ", field, width = width));
            } else {
                line.push_str(&format!("{:<width$} ", field, width = width));
            }
        }
        line.trim_end().to_string()
    }

    fn rule(&self) -> String {
        self.widths
            .iter()
            .enumerate()
            .map(|(i, width)| "-".repeat(if i == 0 { width + 1 } else { width + 2 }))
            .collect::<Vec<_>>()
            .join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tabled::settings::Alignment;

    #[test]
    fn stream_tables_look_like_tables() {
        let header: Vec<String> = ["CF", "Time (ms)", "pm.vbat"].iter().map(|s| s.to_string()).collect();
        let row: Vec<String> = ["CF-01", "8112423", "4.117"].iter().map(|s| s.to_string()).collect();
        let mut expected = table_from_records(&header, &[row.clone()]);
        expected.with(Modify::new(Columns::new(1..)).with(Alignment::right()));
        let expected: Vec<String> = expected.to_string().lines().map(|line| line.trim_end().to_string()).collect();

        // Columns as wide as the widest of header and row, like `table`.
        let streamed = StreamTable { widths: vec![5, 9, 7], right: vec![false, true, true] };
        let header: Vec<&str> = header.iter().map(String::as_str).collect();
        let row: Vec<&str> = row.iter().map(String::as_str).collect();
        assert_eq!(vec![streamed.line(&header), streamed.rule(), streamed.line(&row)], expected);
    }

    #[test]
    fn a_value_that_does_not_fit_widens_its_column() {
        let mut streamed = StreamTable { widths: vec![3, 5], right: vec![false, true] };
        streamed.row(&["a", "123456"]);
        assert_eq!(streamed.widths, vec![3, 6]);
        assert_eq!(streamed.line(&["b", "1.000"]), "b   |  1.000");
    }
}
