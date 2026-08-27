//! 纯文本折行与显示宽度计算：按词断行、悬挂缩进、硬折行（保留样式）。
//! 这些函数不依赖 [App](crate::tui::app::App)，便于独立测试与复用。

use ratatui::{
    style::Style,
    text::{Line, Span},
};

pub(crate) fn display_width(s: &str) -> u16 {
    s.chars()
        .map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 })
        .sum()
}

/// 按显示列宽折行：长行（如启动帮助提示）在面板内换行而非被截断。
/// 优先在空格处断行；无空格的连续串（如中文）按字符硬断；超宽单词同样硬断。
pub(crate) fn wrap_line(text: &str, width: u16) -> Vec<String> {
    let w = width.max(1) as usize;
    let mut rows: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;

    for token in text.split(' ') {
        // 先把可能超过宽度的 token 按字符切成若干 ≤w 的段。
        let mut pieces: Vec<String> = Vec::new();
        let mut buf = String::new();
        let mut bw = 0usize;
        for c in token.chars() {
            let cw = display_width(&c.to_string()) as usize;
            if bw + cw > w && !buf.is_empty() {
                pieces.push(std::mem::take(&mut buf));
                bw = 0;
            }
            buf.push(c);
            bw += cw;
        }
        if !buf.is_empty() {
            pieces.push(buf);
        }

        for piece in pieces {
            let piece_w = display_width(&piece) as usize;
            let sep = if cur.is_empty() { 0usize } else { 1usize };
            if cur_w + sep + piece_w > w && !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            if !cur.is_empty() {
                cur.push(' ');
                cur_w += 1;
            }
            cur.push_str(&piece);
            cur_w += piece_w;
        }
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// 日志行的悬挂缩进折行：首行占满宽度，续行左侧留 `hang` 列空白，
/// 使换行后的文本与首行正文（跳过 `[系统] ` 前缀）竖直对齐。
pub(crate) fn wrap_line_hanging(line: &Line<'static>, width: u16, hang: u16) -> Vec<Line<'static>> {
    let w = width.max(1) as usize;
    let h = (hang as usize).min(w.saturating_sub(1));

    // 展平为字符序列（保留样式）。
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    if chars.is_empty() {
        return vec![Line::default()];
    }

    // 与 wrap_line_spans 相同的分词逻辑（按空格切词，超宽词硬切），
    // 但每个片段的宽度上限取续行可用宽（w-h），保证任何一行都放得下。
    let cont_w = w - h;
    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for (c, st) in chars {
        if c == ' ' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push((c, st));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }

    let mut tokens: Vec<Vec<(char, Style)>> = Vec::new();
    for word in words {
        let ww: usize = word
            .iter()
            .map(|(c, _)| display_width(&c.to_string()) as usize)
            .sum();
        if ww <= cont_w {
            tokens.push(word);
            continue;
        }
        let mut piece: Vec<(char, Style)> = Vec::new();
        let mut pw = 0usize;
        for (c, st) in word {
            let cw = display_width(&c.to_string()) as usize;
            if pw + cw > cont_w && !piece.is_empty() {
                tokens.push(std::mem::take(&mut piece));
                pw = 0;
            }
            piece.push((c, st));
            pw += cw;
        }
        if !piece.is_empty() {
            tokens.push(piece);
        }
    }

    // 贪心放置：首行可用 w，续行可用 cont_w；续行左侧补 hang 列空白。
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut row: Vec<(char, Style)> = Vec::new();
    let mut used = 0usize;
    for token in tokens {
        let tw: usize = token
            .iter()
            .map(|(c, _)| display_width(&c.to_string()) as usize)
            .sum();
        let avail = if rows.is_empty() { w } else { cont_w };
        let sep = if row.is_empty() { 0 } else { 1 };
        if used + sep + tw > avail && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        } else if !row.is_empty() {
            row.push((' ', Style::default()));
            used += 1;
        }
        for cs in token {
            row.push(cs);
        }
        used += tw;
    }
    if !row.is_empty() {
        rows.push(row);
    }

    let pad: Vec<(char, Style)> = " "
        .repeat(h)
        .chars()
        .map(|c| (c, Style::default()))
        .collect();
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            if i == 0 || h == 0 {
                line_from_chars(r)
            } else {
                let mut all = pad.clone();
                all.extend(r);
                line_from_chars(all)
            }
        })
        .collect()
}

/// Verbatim 行的硬折行：按显示宽度在任意字符处切断（CJK 亦可），
/// 不做断词、不吞并空格——用于表格框线等预排版内容。
pub(crate) fn hard_wrap_spans(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let w = (width.max(1)) as usize;
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    if chars.is_empty() {
        return vec![Line::default()];
    }
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut used = 0usize;
    for cs in chars {
        let cw = display_width(&cs.0.to_string()) as usize;
        if used + cw > w && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        used += cw;
        cur.push(cs);
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows.into_iter().map(line_from_chars).collect()
}

/// 把带多段样式的 `Line` 按宽度折行，保留每段原有的颜色/样式。
pub(crate) fn wrap_line_spans(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let w = width.max(1) as usize;
    // 展平为字符序列（保留样式）。
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();

    // 按空格切成「词」，词间用空格连接（与 wrap_line 行为一致）。
    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for (c, st) in chars {
        if c == ' ' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push((c, st));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }

    // 超过宽度的词再按字符切成若干 ≤w 的片段，避免超长行溢出。
    let mut tokens: Vec<Vec<(char, Style)>> = Vec::new();
    for word in words {
        let word_w: usize = word
            .iter()
            .map(|(c, _)| display_width(&c.to_string()) as usize)
            .sum();
        if word_w <= w {
            tokens.push(word);
            continue;
        }
        let mut piece: Vec<(char, Style)> = Vec::new();
        let mut pw = 0usize;
        for (c, st) in word {
            let cw = display_width(&c.to_string()) as usize;
            if pw + cw > w && !piece.is_empty() {
                tokens.push(std::mem::take(&mut piece));
                pw = 0;
            }
            piece.push((c, st));
            pw += cw;
        }
        if !piece.is_empty() {
            tokens.push(piece);
        }
    }

    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut row: Vec<(char, Style)> = Vec::new();
    let mut row_w = 0usize;
    for token in tokens {
        let token_w: usize = token
            .iter()
            .map(|(c, _)| display_width(&c.to_string()) as usize)
            .sum();
        let sep = if row.is_empty() { 0usize } else { 1usize };
        if row_w + sep + token_w > w && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            row_w = 0;
        }
        if !row.is_empty() {
            row.push((' ', Style::default()));
            row_w += 1;
        }
        for cs in token {
            row_w += display_width(&cs.0.to_string()) as usize;
            row.push(cs);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }

    if rows.is_empty() {
        return vec![Line::default()];
    }
    rows.into_iter().map(line_from_chars).collect()
}

/// 把 (字符, 样式) 序列按样式连续段合并为带 Span 的 Line。
pub(crate) fn line_from_chars(chars: Vec<(char, Style)>) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let st = chars[i].1;
        let mut s = String::new();
        s.push(chars[i].0);
        let mut j = i + 1;
        while j < chars.len() && chars[j].1 == st {
            s.push(chars[j].0);
            j += 1;
        }
        spans.push(Span::styled(s, st));
        i = j;
    }
    Line::from(spans)
}
