//! Curl download progress parsing from builder log lines.

use cognos::Host;

/// Curl download progress observed in a derivation's build logs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CurlProgress {
  /// Target host extracted from curl's `trying <url>` line, or a `"curl"`
  /// fallback when progress rows appear before any URL line.
  pub host:              Host,
  /// URL scheme (such as `https` or `http`), when present.
  pub protocol:          Option<String>,
  /// Bytes downloaded so far.
  pub bytes_transferred: u64,
  /// Expected total download size in bytes, once known.
  pub total_bytes:       Option<u64>,
}

impl CurlProgress {
  pub(crate) const fn new(host: Host, protocol: Option<String>) -> Self {
    Self {
      host,
      protocol,
      bytes_transferred: 0,
      total_bytes: None,
    }
  }

  pub(crate) fn fallback() -> Self {
    Self::new(Host::Remote("curl".to_owned()), None)
  }
}

/// Structured curl event extracted from a single builder log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CurlLine {
  /// Target URL announced by nixpkgs `fetchurl` (`trying https://...`).
  TargetUrl {
    host:     Host,
    protocol: Option<String>,
  },
  /// One row of curl's progress meter table.
  Progress { done: u64, total: Option<u64> },
}

/// Parses curl target URL or progress-meter rows from one builder log line.
#[must_use]
pub(crate) fn parse_line(line: &str) -> Option<CurlLine> {
  let clean = strip_ansi(line);
  let trimmed = clean.trim();
  if trimmed.is_empty() {
    return None;
  }

  if let Some((host, protocol)) = parse_trying_url(trimmed) {
    return Some(CurlLine::TargetUrl { host, protocol });
  }

  parse_progress_row(trimmed)
    .map(|(done, total)| CurlLine::Progress { done, total })
}

fn strip_ansi(line: &str) -> String {
  if !line.contains('\x1b') {
    return line.to_owned();
  }
  let mut out = String::with_capacity(line.len());
  let mut chars = line.chars();
  while let Some(character) = chars.next() {
    if character == '\x1b' {
      for next in chars.by_ref() {
        if next.is_ascii_alphabetic() {
          break;
        }
      }
    } else {
      out.push(character);
    }
  }
  out
}

fn parse_trying_url(trimmed: &str) -> Option<(Host, Option<String>)> {
  let url = trimmed.strip_prefix("trying ")?.trim();
  if url.is_empty() {
    return None;
  }

  let (protocol, rest) = url.split_once("://").map_or((None, url), |(p, r)| {
    let protocol = (!p.is_empty()).then(|| p.to_owned());
    (protocol, r)
  });

  let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
  let host_part = authority
    .rsplit_once('@')
    .map_or(authority, |(_, host)| host)
    .trim();

  if host_part.is_empty() {
    return Some((Host::Remote("curl".to_owned()), None));
  }
  if host_part == "localhost" {
    return Some((Host::Localhost, protocol));
  }
  Some((Host::Remote(host_part.to_owned()), protocol))
}

#[expect(
  clippy::cast_sign_loss,
  reason = "negative and non-finite values are rejected before casting"
)]
fn parse_size(token: &str) -> Option<u64> {
  let token = token.trim();
  if token == "--" {
    return Some(0);
  }
  if token.is_empty() {
    return None;
  }
  let last = token.chars().last()?;
  if last.is_ascii_alphabetic() {
    let number_part = &token[..token.len() - last.len_utf8()];
    let value: f64 = number_part.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
      return None;
    }
    let multiplier = match last {
      'k' | 'K' => 1024.0_f64,
      'm' | 'M' => 1024.0_f64 * 1024.0,
      'g' | 'G' => 1024.0_f64 * 1024.0 * 1024.0,
      't' | 'T' => 1024.0_f64 * 1024.0 * 1024.0 * 1024.0,
      'p' | 'P' => 1024.0_f64 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
      _ => return None,
    };
    Some((value * multiplier).round() as u64)
  } else if let Ok(bytes) = token.parse::<u64>() {
    Some(bytes)
  } else if let Ok(value) = token.parse::<f64>()
    && value.is_finite()
    && value >= 0.0
  {
    Some(value.round() as u64)
  } else {
    None
  }
}

fn is_valid_percent(token: &str) -> bool {
  token == "--" || token.parse::<u8>().is_ok_and(|percent| percent <= 100)
}

fn parse_progress_row(trimmed: &str) -> Option<(u64, Option<u64>)> {
  let mut tokens = [""; 14];
  let mut count = 0;
  for token in trimmed.split_whitespace() {
    if count < tokens.len() {
      tokens[count] = token;
    }
    count += 1;
  }
  if !(9..=13).contains(&count) {
    return None;
  }
  if !is_valid_percent(tokens[0]) || !is_valid_percent(tokens[2]) {
    return None;
  }
  let total = parse_size(tokens[1])?;
  let done = parse_size(tokens[3])?;
  Some((done, (total > 0).then_some(total)))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_trying_urls() {
    assert_eq!(
      parse_line(
        "trying https://cdn.geekbench.com/Geekbench-6.7.1-Linux.tar.gz"
      ),
      Some(CurlLine::TargetUrl {
        host:     Host::Remote("cdn.geekbench.com".to_owned()),
        protocol: Some("https".to_owned()),
      })
    );
    assert_eq!(
      parse_line(
        "trying http://user@mirror.example.com:8080/archive.tar.xz?v=1#hash"
      ),
      Some(CurlLine::TargetUrl {
        host:     Host::Remote("mirror.example.com:8080".to_owned()),
        protocol: Some("http".to_owned()),
      })
    );
    assert_eq!(parse_line("trying "), None);
  }

  #[test]
  fn parses_progress_rows_and_retries() {
    assert_eq!(
      parse_line(
        "  0      0   0      0   0      0      0      0                              0"
      ),
      Some(CurlLine::Progress {
        done:  0,
        total: None,
      })
    );
    assert_eq!(
      parse_line(
        "  1 217.5M   1  3.85M   0      0  2.49M      0   01:27   00:01   \
         01:26  3.85M"
      ),
      Some(CurlLine::Progress {
        done:  4_037_018,
        total: Some(228_065_280),
      })
    );
    assert_eq!(
      parse_line(
        " 22 203.0M  22 45.82M   0      0  8.28M      0   00:24   00:05   \
         00:19  9.09M"
      ),
      Some(CurlLine::Progress {
        done:  48_045_752,
        total: Some(212_860_928),
      })
    );
  }

  #[test]
  fn ignores_headers_and_regular_logs() {
    assert_eq!(
      parse_line(
        "  % Total    % Received % Xferd  Average Speed  Time    Time    Time   Current"
      ),
      None
    );
    assert_eq!(
      parse_line(
        "                                 Dload  Upload  Total   Spent   Left   Speed"
      ),
      None
    );
    assert_eq!(
      parse_line(
        "curl: (56) OpenSSL SSL_read: unexpected eof while reading, errno 0"
      ),
      None
    );
    assert_eq!(
      parse_line(
        "Warning: Problem (retrying all errors). Retrying in 1 second."
      ),
      None
    );
  }
}
