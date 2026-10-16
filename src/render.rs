use crate::ui::{paint, printable, styled, width, wrap};

#[derive(Default)]
pub struct Markdown {
    line: String,
    fence: bool,
    table: Vec<String>,
}

impl Markdown {
    pub fn push(&mut self, text: &str) {
        let text = printable(text);
        self.line.push_str(&text);
        if !text.contains('\n') {
            return;
        }
        while let Some(end) = self.line.find('\n') {
            let line: String = self.line.drain(..=end).collect();
            self.render(line.trim_end_matches('\n'));
        }
    }

    pub fn finish(&mut self) {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.render(&line);
        }
        self.flush_table();
        self.fence = false;
    }

    fn render(&mut self, line: &str) {
        if styled() && !self.fence && line.trim_start().starts_with('|') {
            self.table.push(line.trim().to_string());
            return;
        }
        self.flush_table();
        let styled = self.style(line);
        println!("{styled}");
    }

    fn style(&mut self, line: &str) -> String {
        if !styled() {
            return line.to_string();
        }
        let text = line.trim_start();
        let indent = &line[..line.len() - text.len()];
        if let Some(lang) = text.strip_prefix("```") {
            self.fence = !self.fence;
            return paint("2", &if self.fence { format!("  ╭─ {lang}") } else { "  ╰─".into() });
        }
        if self.fence {
            return format!("{}{}", paint("2", "  │ "), paint("36", line));
        }
        let hashes = text.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && text[hashes..].starts_with(' ') {
            let title = inline(text[hashes..].trim()).replace("\x1b[22m", "");
            return paint(if hashes == 1 { "1;4" } else { "1" }, &title);
        }
        if text.len() >= 3 && (text.chars().all(|c| c == '-') || text.chars().all(|c| c == '*')) {
            return paint("2", &"─".repeat(40));
        }
        if let Some(quote) = text.strip_prefix('>') {
            return format!("{}{}", paint("2", "│ "), inline(quote.trim_start()));
        }
        if let Some(item) = text.strip_prefix(['-', '*', '+']).and_then(|t| t.strip_prefix(' ')) {
            return format!("{indent}{} {}", paint("36", "•"), inline(item));
        }
        format!("{indent}{}", inline(text))
    }

    fn flush_table(&mut self) {
        let rows: Vec<Vec<String>> = std::mem::take(&mut self.table)
            .iter()
            .map(|row| row.trim_matches('|').split('|').map(|cell| cell.trim().to_string()).collect())
            .collect();
        let rule = |row: &Vec<String>| row.iter().all(|c| !c.is_empty() && c.chars().all(|c| matches!(c, '-' | ':')));
        let mut widths = vec![];
        for row in rows.iter().filter(|r| !rule(r)) {
            widths.resize(widths.len().max(row.len()), 0);
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(visible(cell));
            }
        }
        let room = width().saturating_sub(3 * widths.len() + 1);
        while widths.iter().sum::<usize>() > room {
            let (i, &widest) = widths.iter().enumerate().max_by_key(|&(_, w)| *w).expect("a table has columns");
            if widest <= 8 {
                break;
            }
            widths[i] = widest - 1;
        }
        let bar = paint("2", "│");
        for row in &rows {
            if rule(row) {
                let dashes: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
                println!("{}", paint("2", &format!("├{}┤", dashes.join("┼"))));
                continue;
            }
            let cells: Vec<Vec<String>> =
                widths.iter().enumerate().map(|(i, &w)| wrap(row.get(i).map_or("", String::as_str), w)).collect();
            for line in 0..cells.iter().map(Vec::len).max().unwrap_or(1) {
                let shown: Vec<String> = cells
                    .iter()
                    .zip(&widths)
                    .map(|(cell, &w)| {
                        let text = cell.get(line).map_or("", String::as_str);
                        format!(" {}{} ", inline(text), " ".repeat(w.saturating_sub(visible(text))))
                    })
                    .collect();
                println!("{bar}{}{bar}", shown.join(&bar));
            }
        }
    }
}

fn visible(cell: &str) -> usize {
    let mut plain = inline(cell);
    for esc in ["\x1b[36m", "\x1b[39m", "\x1b[1m", "\x1b[22m"] {
        plain = plain.replace(esc, "");
    }
    plain.chars().count()
}

pub fn inline(text: &str) -> String {
    let (mut out, mut bold, mut code) = (String::new(), false, false);
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '`' && (code || rest[1..].contains('`')) {
            code = !code;
            out += if code { "\x1b[36m" } else { "\x1b[39m" };
            rest = &rest[1..];
        } else if !code && rest.starts_with("**") && (bold || rest[2..].contains("**")) {
            bold = !bold;
            out += if bold { "\x1b[1m" } else { "\x1b[22m" };
            rest = &rest[2..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}
