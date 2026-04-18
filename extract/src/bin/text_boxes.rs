use anyhow::{Context, Result};
use lopdf::{
    Document, Encoding, Object,
    content::{Content, Operation},
};
use serde::Serialize;
use std::{collections::BTreeMap, env, path::PathBuf};

#[derive(Serialize)]
struct Page {
    number: u32,
    text_fields: Vec<TextField>,
}

#[derive(Serialize)]
struct TextField {
    bbox: [f32; 4],
    text: String,
}

fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("Usage: text_boxes <pdf-path>")?;

    let doc = Document::load(&path)?;
    let pages = doc.get_pages();
    let mut output_pages = Vec::new();

    for (page_number, page_id) in pages {
        let fonts = doc.get_page_fonts(page_id)?;
        let font_metrics = build_font_metrics(&doc, &fonts);
        let content = doc.get_and_decode_page_content(page_id)?;
        let entries = extract_text_entries(&doc, &font_metrics, &content);
        let text_fields = fuse_text_entries(entries);
        output_pages.push(Page {
            number: page_number,
            text_fields,
        });
    }

    let yaml = serde_yaml::to_string(&output_pages)?;
    println!("{}", yaml);
    Ok(())
}

struct TextEntry {
    text: String,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

#[derive(Debug)]
struct FontMetrics<'a> {
    encoding: Option<Encoding<'a>>,
    widths: Vec<f32>,
    first_char: i64,
    ascent: f32,
    descent: f32,
}

impl<'a> FontMetrics<'a> {
    fn from_font_dict(font: &'a lopdf::Dictionary, doc: &'a Document) -> Self {
        let encoding = font.get_font_encoding(doc).ok();
        let first_char = font.get(b"FirstChar").and_then(Object::as_i64).unwrap_or(0);
        let widths = font
            .get(b"Widths")
            .and_then(Object::as_array)
            .map(|array| {
                array
                    .iter()
                    .map(|obj| obj.as_f32().unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();

        let (ascent, descent) = Self::font_metrics_from_descriptor(font, doc);
        FontMetrics {
            encoding,
            widths,
            first_char,
            ascent,
            descent,
        }
    }

    fn font_metrics_from_descriptor(font: &lopdf::Dictionary, doc: &Document) -> (f32, f32) {
        if let Ok(descriptor_obj) = font.get(b"FontDescriptor")
            && let Ok((_, descriptor)) = doc.dereference(descriptor_obj)
            && let Ok(descriptor_dict) = descriptor.as_dict()
        {
            let ascent = descriptor_dict
                .get(b"Ascent")
                .and_then(Object::as_f32)
                .unwrap_or(0.0);
            let descent = descriptor_dict
                .get(b"Descent")
                .and_then(Object::as_f32)
                .unwrap_or(0.0);
            if ascent != 0.0 || descent != 0.0 {
                return (ascent, descent);
            }
        }
        (700.0, -200.0)
    }

    fn decode_text(&self, bytes: &[u8]) -> String {
        if let Some(encoding) = &self.encoding {
            encoding
                .bytes_to_string(bytes)
                .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string())
        } else {
            String::from_utf8_lossy(bytes).to_string()
        }
    }

    fn width_for_byte(&self, byte: u8) -> f32 {
        let index = byte as i64 - self.first_char;
        if index >= 0 && (index as usize) < self.widths.len() {
            self.widths[index as usize] / 1000.0
        } else {
            0.5
        }
    }

    fn width_for_bytes(&self, bytes: &[u8], char_space: f32, word_space: f32) -> f32 {
        let mut width = 0.0;
        let mut first = true;
        for &byte in bytes {
            if !first {
                width += char_space;
            }
            if byte == b' ' {
                width += word_space;
            }
            width += self.width_for_byte(byte);
            first = false;
        }
        width
    }
}

#[derive(Clone, Debug)]
struct TextState {
    tm: [f32; 6],
    tlm: [f32; 6],
    font_size: f32,
    hscale: f32,
    char_space: f32,
    word_space: f32,
    leading: f32,
    rise: f32,
    font_name: Option<Vec<u8>>,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            tm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            tlm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            font_size: 12.0,
            hscale: 1.0,
            char_space: 0.0,
            word_space: 0.0,
            leading: 0.0,
            rise: 0.0,
            font_name: None,
        }
    }
}

fn build_font_metrics<'a>(
    doc: &'a Document,
    fonts: &BTreeMap<Vec<u8>, &'a lopdf::Dictionary>,
) -> BTreeMap<Vec<u8>, FontMetrics<'a>> {
    fonts
        .iter()
        .map(|(name, font)| (name.clone(), FontMetrics::from_font_dict(font, doc)))
        .collect()
}

fn extract_text_entries(
    doc: &Document,
    fonts: &BTreeMap<Vec<u8>, FontMetrics>,
    content: &Content<Vec<Operation>>,
) -> Vec<TextEntry> {
    let mut entries = Vec::new();
    let mut state = TextState::default();
    let mut in_text = false;

    let operations: &[Operation] = content.operations.as_ref();
    for operation in operations {
        match operation.operator.as_str() {
            "BT" => {
                in_text = true;
                state = TextState::default();
            }
            "ET" => {
                in_text = false;
            }
            "Tm" if in_text => {
                if let Some(matrix) = parse_matrix(doc, &operation.operands) {
                    state.tlm = matrix;
                    state.tm = matrix;
                }
            }
            "Td" if in_text => {
                if let Some((tx, ty)) = parse_two_numbers(doc, &operation.operands) {
                    state.tlm = multiply_matrix(state.tlm, [1.0, 0.0, 0.0, 1.0, tx, ty]);
                    state.tm = state.tlm;
                }
            }
            "TD" if in_text => {
                if let Some((tx, ty)) = parse_two_numbers(doc, &operation.operands) {
                    state.leading = -ty;
                    state.tlm = multiply_matrix(state.tlm, [1.0, 0.0, 0.0, 1.0, tx, ty]);
                    state.tm = state.tlm;
                }
            }
            "T*" if in_text => {
                state.tlm = multiply_matrix(state.tlm, [1.0, 0.0, 0.0, 1.0, 0.0, -state.leading]);
                state.tm = state.tlm;
            }
            "Tf" if in_text => {
                if let Some(font_name) = operation
                    .operands
                    .first()
                    .and_then(|operand| get_name(doc, operand))
                {
                    state.font_name = Some(font_name.to_vec());
                }
                if let Some(size_obj) = operation.operands.get(1)
                    && let Ok(size) = get_number(doc, size_obj)
                {
                    state.font_size = size;
                }
            }
            "Tc" if in_text => {
                if let Some(value) = operation
                    .operands
                    .first()
                    .and_then(|operand| get_number(doc, operand).ok())
                {
                    state.char_space = value;
                }
            }
            "Tw" if in_text => {
                if let Some(value) = operation
                    .operands
                    .first()
                    .and_then(|operand| get_number(doc, operand).ok())
                {
                    state.word_space = value;
                }
            }
            "Th" if in_text => {
                if let Some(value) = operation
                    .operands
                    .first()
                    .and_then(|operand| get_number(doc, operand).ok())
                {
                    state.hscale = value / 100.0;
                }
            }
            "TL" if in_text => {
                if let Some(value) = operation
                    .operands
                    .first()
                    .and_then(|operand| get_number(doc, operand).ok())
                {
                    state.leading = value;
                }
            }
            "Ts" if in_text => {
                if let Some(value) = operation
                    .operands
                    .first()
                    .and_then(|operand| get_number(doc, operand).ok())
                {
                    state.rise = value;
                }
            }
            "Tj" if in_text => {
                if let Some(text_obj) = operation.operands.first()
                    && let Some(entry) = make_text_entry(doc, &state, text_obj, fonts)
                {
                    entries.push(entry);
                    let width = calculate_text_width(doc, &state, text_obj, fonts);
                    state.tm = multiply_matrix(state.tm, [1.0, 0.0, 0.0, 1.0, width, 0.0]);
                }
            }
            "TJ" if in_text => {
                if let Some(array_obj) = operation.operands.first()
                    && let Some(entry) = make_text_entry(doc, &state, array_obj, fonts)
                {
                    entries.push(entry);
                    let width = calculate_text_width(doc, &state, array_obj, fonts);
                    state.tm = multiply_matrix(state.tm, [1.0, 0.0, 0.0, 1.0, width, 0.0]);
                }
            }
            _ => {}
        }
    }

    entries
}

fn fuse_text_entries(mut entries: Vec<TextEntry>) -> Vec<TextField> {
    if entries.is_empty() {
        return Vec::new();
    }

    // Sort by y1 (top), then by x1 (left)
    entries.sort_by(|a, b| {
        a.y1.partial_cmp(&b.y1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x1.partial_cmp(&b.x1).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut fields = Vec::new();
    let mut current_line: Vec<TextEntry> = Vec::new();

    for entry in entries {
        if current_line.is_empty() {
            current_line.push(entry);
        } else {
            let last = current_line.last().unwrap();
            // Check if on same line (y overlap) and close in x
            let y_overlap = entry.y1 < last.y2 && entry.y2 > last.y1;
            let x_close = last.x1 <= entry.x1 && entry.x1 <= last.x2 + 5.0; // 5 units threshold
            if y_overlap && x_close {
                current_line.push(entry);
            } else {
                // Fuse current line
                fields.push(fuse_line(&current_line));
                current_line = vec![entry];
            }
        }
    }
    if !current_line.is_empty() {
        fields.push(fuse_line(&current_line));
    }

    fields
}

fn fuse_line(line: &[TextEntry]) -> TextField {
    let mut text = String::new();
    let mut x1 = f32::INFINITY;
    let mut y1 = f32::INFINITY;
    let mut x2 = f32::NEG_INFINITY;
    let mut y2 = f32::NEG_INFINITY;

    for entry in line {
        text.push_str(&entry.text);
        x1 = x1.min(entry.x1);
        y1 = y1.min(entry.y1);
        x2 = x2.max(entry.x2);
        y2 = y2.max(entry.y2);
    }

    TextField {
        bbox: [x1, y1, x2, y2],
        text,
    }
}

fn make_text_entry(
    doc: &Document,
    state: &TextState,
    text_obj: &Object,
    fonts: &BTreeMap<Vec<u8>, FontMetrics>,
) -> Option<TextEntry> {
    let (text, length) = if let Ok(string_bytes) = extract_bytes(doc, text_obj) {
        let (decoded, width) = decode_and_width(string_bytes, state, fonts);
        (decoded, width)
    } else if let Ok((_, obj)) = doc.dereference(text_obj) {
        if let Ok(array) = obj.as_array() {
            let (decoded, width) = decode_and_width_array(doc, array, state, fonts);
            (decoded, width)
        } else {
            return None;
        }
    } else {
        return None;
    };

    let bbox = make_bbox(state, length, fonts);
    Some(TextEntry {
        text,
        x1: bbox[0],
        y1: bbox[1],
        x2: bbox[2],
        y2: bbox[3],
    })
}

fn calculate_text_width(
    doc: &Document,
    state: &TextState,
    text_obj: &Object,
    fonts: &BTreeMap<Vec<u8>, FontMetrics>,
) -> f32 {
    if let Ok(string_bytes) = extract_bytes(doc, text_obj) {
        width_for_bytes(string_bytes, state, fonts)
    } else if let Ok((_, obj)) = doc.dereference(text_obj) {
        if let Ok(array) = obj.as_array() {
            width_for_array(doc, array, state, fonts)
        } else {
            0.0
        }
    } else {
        0.0
    }
}

fn width_for_array(
    doc: &Document,
    array: &[Object],
    state: &TextState,
    fonts: &BTreeMap<Vec<u8>, FontMetrics>,
) -> f32 {
    let mut width = 0.0;
    for item in array {
        if let Ok(string_bytes) = extract_bytes(doc, item) {
            width += width_for_bytes(string_bytes, state, fonts);
        } else if let Ok(adjustment) = get_number(doc, item) {
            width -= adjustment / 1000.0 * state.font_size * state.hscale;
        }
    }
    width
}

fn decode_and_width(
    bytes: &[u8],
    state: &TextState,
    fonts: &BTreeMap<Vec<u8>, FontMetrics>,
) -> (String, f32) {
    if let Some(name) = &state.font_name
        && let Some(metrics) = fonts.get(name)
    {
        let text = metrics.decode_text(bytes);
        let width = metrics.width_for_bytes(bytes, state.char_space, state.word_space)
            * state.font_size
            * state.hscale;
        return (text, width);
    }
    (
        String::from_utf8_lossy(bytes).to_string(),
        bytes.len() as f32 * state.font_size * state.hscale * 0.5,
    )
}

fn decode_and_width_array(
    doc: &Document,
    array: &[Object],
    state: &TextState,
    fonts: &BTreeMap<Vec<u8>, FontMetrics>,
) -> (String, f32) {
    let mut text = String::new();
    let mut width = 0.0;
    for item in array {
        if let Ok(bytes) = extract_bytes(doc, item) {
            let (decoded, chunk_width) = decode_and_width(bytes, state, fonts);
            text.push_str(&decoded);
            width += chunk_width;
        } else if let Ok(adjustment) = get_number(doc, item) {
            width -= adjustment / 1000.0 * state.font_size * state.hscale;
        }
    }
    (text, width)
}

fn width_for_bytes(bytes: &[u8], state: &TextState, fonts: &BTreeMap<Vec<u8>, FontMetrics>) -> f32 {
    if let Some(name) = &state.font_name
        && let Some(metrics) = fonts.get(name)
    {
        return metrics.width_for_bytes(bytes, state.char_space, state.word_space)
            * state.font_size
            * state.hscale;
    }
    bytes.len() as f32 * state.font_size * state.hscale * 0.5
}

fn extract_bytes<'a>(doc: &'a Document, object: &'a Object) -> Result<&'a [u8]> {
    let (_, obj) = doc.dereference(object)?;
    match obj {
        Object::String(bytes, _) => Ok(bytes),
        _ => Err(anyhow::anyhow!("Expected string object")),
    }
}

fn get_name<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a [u8]> {
    if let Ok((_, obj)) = doc.dereference(object) {
        obj.as_name().ok()
    } else {
        None
    }
}

fn get_number(doc: &Document, object: &Object) -> Result<f32> {
    let (_, object) = doc.dereference(object)?;
    match object {
        Object::Integer(value) => Ok(*value as f32),
        Object::Real(value) => Ok(*value),
        _ => Err(anyhow::anyhow!("Expected numeric object")),
    }
}

fn parse_matrix(doc: &Document, operands: &[Object]) -> Option<[f32; 6]> {
    if operands.len() == 6 {
        let numbers = operands
            .iter()
            .filter_map(|operand| get_number(doc, operand).ok())
            .collect::<Vec<_>>();
        if numbers.len() == 6 {
            return Some([
                numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
            ]);
        }
    }
    None
}

fn parse_two_numbers(doc: &Document, operands: &[Object]) -> Option<(f32, f32)> {
    if operands.len() >= 2
        && let (Ok(tx), Ok(ty)) = (get_number(doc, &operands[0]), get_number(doc, &operands[1]))
    {
        return Some((tx, ty));
    }
    None
}

fn multiply_matrix(first: [f32; 6], second: [f32; 6]) -> [f32; 6] {
    [
        first[0] * second[0] + first[2] * second[1],
        first[1] * second[0] + first[3] * second[1],
        first[0] * second[2] + first[2] * second[3],
        first[1] * second[2] + first[3] * second[3],
        first[0] * second[4] + first[2] * second[5] + first[4],
        first[1] * second[4] + first[3] * second[5] + first[5],
    ]
}

fn tm_transform(tm: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (tm[0] * x + tm[2] * y + tm[4], tm[1] * x + tm[3] * y + tm[5])
}

fn make_bbox(state: &TextState, width: f32, fonts: &BTreeMap<Vec<u8>, FontMetrics>) -> [f32; 4] {
    let (ascent, descent) = font_extents(state, fonts);
    let origin = tm_transform(state.tm, 0.0, state.rise * state.font_size);
    let end = tm_transform(state.tm, width, state.rise * state.font_size);
    let top = tm_transform(state.tm, 0.0, state.rise * state.font_size + ascent);
    let top_right = tm_transform(state.tm, width, state.rise * state.font_size + ascent);
    let bottom = tm_transform(state.tm, 0.0, state.rise * state.font_size + descent);
    let bottom_right = tm_transform(state.tm, width, state.rise * state.font_size + descent);

    let xs = [
        origin.0,
        end.0,
        top.0,
        top_right.0,
        bottom.0,
        bottom_right.0,
    ];
    let ys = [
        origin.1,
        end.1,
        top.1,
        top_right.1,
        bottom.1,
        bottom_right.1,
    ];
    [
        xs.iter().cloned().fold(f32::INFINITY, f32::min),
        ys.iter().cloned().fold(f32::INFINITY, f32::min),
        xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
    ]
}

fn font_extents(state: &TextState, fonts: &BTreeMap<Vec<u8>, FontMetrics>) -> (f32, f32) {
    if let Some(name) = &state.font_name
        && let Some(metrics) = fonts.get(name)
    {
        return (
            metrics.ascent * state.font_size / 1000.0,
            metrics.descent * state.font_size / 1000.0,
        );
    }
    (
        700.0 * state.font_size / 1000.0,
        -200.0 * state.font_size / 1000.0,
    )
}
