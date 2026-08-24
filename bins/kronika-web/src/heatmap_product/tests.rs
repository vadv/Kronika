use super::{HeatmapConversion, HeatmapProductError};

#[test]
fn registry_keeps_the_web_surface_defaults() {
    let surfaces = super::surfaces().expect("Heatmap product registry");
    let defaults = surfaces
        .iter()
        .map(|surface| {
            (
                surface.id.as_str(),
                surface.default_cut.as_str(),
                surface.default_group.as_str(),
                surface.default_columns,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        defaults,
        [
            ("processes", "cpu", "command", 60),
            ("statements", "exec_time", "identity", 60),
            ("plans", "exec_time", "identity", 60),
            ("databases", "commits", "identity", 60),
            ("tables", "writes", "identity", 12),
            ("indexes", "idx_scan", "identity", 12),
            ("cgroups", "cg_cpu", "identity", 60),
        ]
    );
    assert_eq!(super::policy().expect("Heatmap policy").default_top, 25);
}

#[test]
fn product_selection_resolves_one_physical_recipe() {
    let processes = super::resolve("processes", None, None, None).expect("Process Heatmap");
    assert_eq!(processes.cut.section, "os_process");
    assert_eq!(processes.cut.fields, ["utime", "stime"]);
    assert_eq!(processes.group.fields, ["comm"]);
    assert_eq!(processes.columns, 60);
    assert!(matches!(
        processes.cut.conversion,
        HeatmapConversion::RecordedDivide { ref locator, .. }
            if locator == "instance_metadata.clock_ticks_per_sec"
    ));

    let tables =
        super::resolve("tables", Some("writes"), Some("schema"), None).expect("Table Heatmap");
    assert_eq!(tables.cut.section, "pg_stat_user_tables");
    assert_eq!(tables.cut.fields, ["n_tup_ins", "n_tup_upd", "n_tup_del"]);
    assert_eq!(tables.group.fields, ["datname", "schemaname"]);
    assert_eq!(tables.columns, 12);
}

#[test]
fn product_selection_rejects_cross_surface_values_and_bounds() {
    assert_eq!(
        super::resolve("tables", Some("cpu"), None, None).err(),
        Some(HeatmapProductError::Cut)
    );
    assert_eq!(
        super::resolve("processes", None, Some("schema"), None).err(),
        Some(HeatmapProductError::Group)
    );
    assert_eq!(
        super::resolve("processes", None, None, Some(0)).err(),
        Some(HeatmapProductError::Columns)
    );
}
