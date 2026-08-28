use std::io::{self, Write};

use ratatui_core::{
  buffer::Buffer,
  style::{Color, Modifier},
};

use crate::{
  state::State,
  types::{LogLine, RenderConfig},
};

/// A fully materialized presentation.
#[derive(Debug, Clone)]
pub struct Frame {
  pub buffer: Buffer,
  pub height: u16,
}

impl Frame {
  /// Return a stable, style-free representation used by fixture goldens.
  #[must_use]
  pub fn text(&self) -> String {
    let area = self.buffer.area;
    (0..self.height)
      .map(|y| {
        let mut line = String::new();
        for x in 0..area.width {
          if let Some(cell) = self.buffer.cell((x, y)) {
            line.push_str(cell.symbol());
          }
        }
        line.trim_end().to_string()
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  /// Return the same frame as append-only text with exact ANSI SGR styling.
  /// No cursor movement, clearing, or terminal-mode controls are emitted.
  #[must_use]
  pub fn ansi_text(&self) -> String {
    let area = self.buffer.area;
    let mut output = String::new();
    for y in 0..self.height {
      let last = (0..area.width).rfind(|&x| {
        self.buffer.cell((x, y)).is_some_and(|cell| {
          let symbol = cell.symbol();
          (!symbol.is_empty() && symbol != " ")
            || cell.fg != Color::Reset
            || cell.bg != Color::Reset
            || !cell.modifier.is_empty()
        })
      });
      let mut style = (Color::Reset, Color::Reset, Modifier::empty());
      if let Some(last) = last {
        for x in 0..=last {
          let Some(cell) = self.buffer.cell((x, y)) else {
            continue;
          };
          let next_style = (cell.fg, cell.bg, cell.modifier);
          if next_style != style {
            output.push_str(&ansi_style(
              next_style.0,
              next_style.1,
              next_style.2,
            ));
            style = next_style;
          }
          output.push_str(cell.symbol());
        }
      }
      if style != (Color::Reset, Color::Reset, Modifier::empty()) {
        output.push_str("\x1b[0m");
      }
      if y + 1 < self.height {
        output.push('\n');
      }
    }
    output
  }
}

fn ansi_style(
  foreground: Color,
  background: Color,
  modifiers: Modifier,
) -> String {
  let mut codes = vec!["0".to_string()];
  if let Some(code) = ansi_color_code(foreground, false) {
    codes.push(code);
  }
  if let Some(code) = ansi_color_code(background, true) {
    codes.push(code);
  }
  for (modifier, code) in [
    (Modifier::BOLD, "1"),
    (Modifier::DIM, "2"),
    (Modifier::ITALIC, "3"),
    (Modifier::UNDERLINED, "4"),
    (Modifier::SLOW_BLINK, "5"),
    (Modifier::RAPID_BLINK, "6"),
    (Modifier::REVERSED, "7"),
    (Modifier::HIDDEN, "8"),
    (Modifier::CROSSED_OUT, "9"),
  ] {
    if modifiers.contains(modifier) {
      codes.push(code.to_string());
    }
  }
  format!("\x1b[{}m", codes.join(";"))
}

fn ansi_foreground(color: Color) -> String {
  ansi_style(color, Color::Reset, Modifier::empty())
}

#[must_use]
pub fn format_log(log: &LogLine, config: &RenderConfig) -> String {
  if !config.ansi {
    return format!("{}{}", strip_ansi(&log.prefix), strip_ansi(&log.plain));
  }
  let mut output = String::new();
  if !log.prefix.is_empty() {
    output.push_str(&ansi_foreground(config.theme.log_prefix));
    output.push_str(&log.prefix);
    output.push_str("\x1b[0m");
  }
  if log.styled.is_empty() {
    output.push_str(&log.plain);
  } else {
    output.push_str(&log.styled);
  }
  output.push_str("\x1b[0m");
  output
}

fn strip_ansi(value: &str) -> String {
  let bytes = value.as_bytes();
  let mut output = String::with_capacity(value.len());
  let mut index = 0;
  let mut plain_start = 0;
  while index < bytes.len() {
    if bytes[index] != 0x1B {
      index += 1;
      continue;
    }
    output.push_str(&value[plain_start..index]);
    index += 1;
    match bytes.get(index) {
      Some(b'[') => {
        index += 1;
        while let Some(byte) = bytes.get(index) {
          index += 1;
          if (0x40..=0x7E).contains(byte) {
            break;
          }
        }
      },
      Some(b']') => {
        index += 1;
        while index < bytes.len() {
          if bytes[index] == 0x07 {
            index += 1;
            break;
          }
          if bytes[index] == 0x1B && bytes.get(index + 1) == Some(&b'\\') {
            index += 2;
            break;
          }
          index += 1;
        }
      },
      Some(_) => index += 1,
      None => {},
    }
    plain_start = index;
  }
  output.push_str(&value[plain_start..]);
  output
}

fn ansi_color_code(color: Color, background: bool) -> Option<String> {
  let offset = u8::from(background) * 10;
  let code = match color {
    Color::Reset => return None,
    Color::Black => 30 + offset,
    Color::Red => 31 + offset,
    Color::Green => 32 + offset,
    Color::Yellow => 33 + offset,
    Color::Blue => 34 + offset,
    Color::Magenta => 35 + offset,
    Color::Cyan => 36 + offset,
    Color::Gray => 37 + offset,
    Color::DarkGray => 90 + offset,
    Color::LightRed => 91 + offset,
    Color::LightGreen => 92 + offset,
    Color::LightYellow => 93 + offset,
    Color::LightBlue => 94 + offset,
    Color::LightMagenta => 95 + offset,
    Color::LightCyan => 96 + offset,
    Color::White => 97 + offset,
    Color::Rgb(red, green, blue) => {
      return Some(format!(
        "{};2;{red};{green};{blue}",
        if background { 48 } else { 38 }
      ));
    },
    Color::Indexed(index) => {
      return Some(format!("{};5;{index}", if background { 48 } else { 38 }));
    },
  };
  Some(code.to_string())
}

pub fn write_final<W: Write>(
  writer: &mut W,
  state: &State,
  config: &RenderConfig,
  now: f64,
) -> io::Result<()> {
  let width = config.width.unwrap_or(100).max(2) - 1;
  let height = config.height.unwrap_or(100).max(1);
  let frame = super::render_frame(state, config, now, width, height, true);
  if config.ansi {
    writeln!(writer, "{}", frame.ansi_text())?;
  } else {
    writeln!(writer, "{}", frame.text())?;
  }
  writer.flush()
}
