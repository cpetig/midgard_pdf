# Extract text from MOAM PDF document

```
cd extract
cargo run --bin text_boxes input.pdf >input.yaml
```

# Paste values into M5 PDF Form

```
cd extract
cargo run --bin fill_form iput.yaml form0.pdf
```

This assumes that the order of pages is the same for MOAM and the pdf form (unlikely)
