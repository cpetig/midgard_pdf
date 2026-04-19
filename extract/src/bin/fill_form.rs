use anyhow::{Context, Result};
use lopdf::{Document, Object};
use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Deserialize)]
struct Page {
    number: u32,
    text_fields: Vec<TextField>,
}

#[derive(Deserialize)]
struct TextField {
    bbox: [f32; 4], // [x1, y1, x2, y2]
    text: String,
}

fn main() -> Result<()> {
    let yaml_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("Usage: fill_form <yaml-path> <pdf-path> [output-path]")?;
    let pdf_path = env::args()
        .nth(2)
        .map(PathBuf::from)
        .context("Usage: fill_form <yaml-path> <pdf-path> [output-path]")?;

    let output_path = env::args()
        .nth(3)
        .map(PathBuf::from)
        .or_else(|| {
            let stem = pdf_path.file_stem()?;
            let ext = pdf_path.extension()?;
            let mut output = stem.to_os_string();
            output.push("_filled.");
            output.push(ext);
            Some(
                pdf_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(output),
            )
        })
        .context("Could not determine output path")?;

    let yaml_content = fs::read_to_string(&yaml_path)?;
    let pages: Vec<Page> = serde_yaml::from_str(&yaml_content)?;

    let mut doc = Document::load(&pdf_path)?;

    let mut field_refs = Vec::new();
    {
        let catalog = doc.catalog()?;
        let acroform_obj = catalog.get_deref(b"AcroForm", &doc)?;
        if let Ok(acroform_dict) = acroform_obj.as_dict() {
            let fields_obj = acroform_dict.get_deref(b"Fields", &doc)?;
            if let Ok(fields_array) = fields_obj.as_array() {
                for field_ref in fields_array {
                    field_refs.push(field_ref.clone());
                }
            }
        }
    }

    for field_ref in field_refs {
        fill_field(&mut doc, &field_ref, &pages)?;
    }

    doc.save(&output_path)?;
    println!("Form filled and saved to {}", output_path.display());
    Ok(())
}

fn fill_field(doc: &mut Document, field_ref: &Object, pages: &[Page]) -> Result<()> {
    if let Object::Reference(field_id) = field_ref {
        let field_obj = doc.get_object(*field_id)?;
        if let Ok(field_dict) = field_obj.as_dict() {
            // Get field name
            let name_obj = match field_dict.get_deref(b"T", doc) {
                Ok(obj) => obj,
                Err(_) => return Ok(()),
            };
            let name = if let Ok(name_str) = name_obj.as_str() {
                String::from_utf8_lossy(name_str).to_string()
            } else {
                return Ok(());
            };
            eprintln!("Processing field: {}", name);
            // Get rect - scale from PDF points to pixels (150/72) and swap x/y
            let rect_obj = match field_dict.get_deref(b"Rect", doc) {
                Ok(obj) => obj,
                Err(_) => return Ok(()),
            };
            let rect = if let Ok(rect_array) = rect_obj.as_array() {
                if rect_array.len() == 4 {
                    let val0 = get_number(doc, &rect_array[0])?;
                    let val1 = get_number(doc, &rect_array[1])?;
                    let val2 = get_number(doc, &rect_array[2])?;
                    let val3 = get_number(doc, &rect_array[3])?;

                    // Scale from PDF points (72 DPI) to pixels at 150 DPI: multiply by 150/72
                    let scale = 150.0 / 72.0;
                    let scaled0 = val0 * scale;
                    let scaled1 = val1 * scale;
                    let scaled2 = val2 * scale;
                    let scaled3 = val3 * scale;

                    // Array is [y_min, x_min, y_max, x_max], rearrange to [x1, y1, x2, y2]
                    [scaled1, scaled0, scaled3, scaled2]
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            };

            // Get page number
            let page_num = if let Ok(Object::Reference(page_id)) = field_dict.get_deref(b"P", doc) {
                let pages_map = doc.get_pages();
                let mut page_num = 1;
                for (num, id) in pages_map {
                    if id == *page_id {
                        page_num = num;
                        break;
                    }
                }
                page_num
            } else {
                1 // assume page 1 if not specified
            };

            // Find matching text field
            if let Some(page) = pages.iter().find(|p| p.number == page_num) {
                let mut best_match = None;
                let mut max_overlap = 0.0;
                for text_field in &page.text_fields {
                    let overlap = compute_overlap(rect, text_field.bbox);
                    if overlap > max_overlap {
                        max_overlap = overlap;
                        best_match = Some(text_field);
                    }
                }
                if let Some(text_field) = best_match {
                    eprintln!(
                        "  ✓ Matched: '{}' (overlap: {:.0})",
                        text_field.text, max_overlap
                    );
                    // Set field value - encode as UTF-16BE for proper Unicode support in PDF
                    let mut utf16_bytes = vec![0xFE, 0xFF]; // UTF-16BE BOM
                    for ch in text_field.text.chars() {
                        let mut buf = [0u16; 2];
                        let encoded = ch.encode_utf16(&mut buf);
                        for u16_val in encoded {
                            utf16_bytes.push(((*u16_val >> 8) & 0xFF) as u8);
                            utf16_bytes.push((*u16_val & 0xFF) as u8);
                        }
                    }
                    let value_obj = Object::String(utf16_bytes, lopdf::StringFormat::Literal);
                    let mut new_dict = field_dict.clone();
                    new_dict.set(b"V".to_vec(), value_obj);
                    doc.set_object(*field_id, Object::Dictionary(new_dict));
                } else {
                    eprintln!("  ✗ No match");
                }
            } else {
                eprintln!("  ✗ Page {} not in YAML", page_num);
            }
        }
    }
    Ok(())
}

fn compute_overlap(rect1: [f32; 4], rect2: [f32; 4]) -> f32 {
    let x1 = rect1[0].max(rect2[0]);
    let y1 = rect1[1].max(rect2[1]);
    let x2 = rect1[2].min(rect2[2]);
    let y2 = rect1[3].min(rect2[3]);
    if x1 < x2 && y1 < y2 {
        (x2 - x1) * (y2 - y1)
    } else {
        0.0
    }
}

fn get_number(doc: &Document, obj: &Object) -> Result<f32> {
    match obj {
        Object::Integer(i) => Ok(*i as f32),
        Object::Real(r) => Ok(*r),
        Object::Reference(r) => {
            let obj = doc.get_object(*r)?;
            get_number(doc, obj)
        }
        _ => Err(anyhow::anyhow!("Not a number")),
    }
}
