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
        .context("Usage: fill_form <yaml-path> <pdf-path>")?;
    let pdf_path = env::args()
        .nth(2)
        .map(PathBuf::from)
        .context("Usage: fill_form <yaml-path> <pdf-path>")?;

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

    doc.save(&pdf_path)?;
    println!("Form filled and saved.");
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
            let _name = if let Ok(name_str) = name_obj.as_str() {
                String::from_utf8_lossy(name_str).to_string()
            } else {
                return Ok(());
            };

            // Get rect
            let rect_obj = match field_dict.get_deref(b"Rect", doc) {
                Ok(obj) => obj,
                Err(_) => return Ok(()),
            };
            let rect = if let Ok(rect_array) = rect_obj.as_array() {
                if rect_array.len() == 4 {
                    let x1 = get_number(doc, &rect_array[0])?;
                    let y1 = get_number(doc, &rect_array[1])?;
                    let x2 = get_number(doc, &rect_array[2])?;
                    let y2 = get_number(doc, &rect_array[3])?;
                    [x1, y1, x2, y2]
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
                    // Set field value
                    let value_obj = Object::String(
                        text_field.text.as_bytes().to_vec(),
                        lopdf::StringFormat::Literal,
                    );
                    let mut new_dict = field_dict.clone();
                    new_dict.set(b"V".to_vec(), value_obj);
                    doc.set_object(*field_id, Object::Dictionary(new_dict));
                }
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
