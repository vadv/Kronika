use super::{FIELDS, layout};

#[test]
fn fixed_layout_keeps_surface_before_population_facts() {
    let layout = layout();
    let columns = layout["columns"].as_array().expect("summary columns");
    assert_eq!(columns.len(), FIELDS.split_ascii_whitespace().count() + 1);
    assert_eq!(columns[0]["name"], "surface");
    assert_eq!(columns.last().expect("last fact")["name"], "usable_pct");
}
