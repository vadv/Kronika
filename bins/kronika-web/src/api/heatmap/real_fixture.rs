//! Build-only bridge from the retained real-hour source to product Heatmap output.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail};
use flate2::read::GzDecoder;
use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_registry::os_process::OsProcess;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Value, json};

use super::{HeatmapRequest, prepare};

const HOUR_MICROS: i64 = 3_600_000_000;
const PROCESS_TYPE_ID: &str = "1100001";
const COLUMNS: usize = 60;
const TOPS: [usize; 4] = [10, 25, 50, 100];
const PROCESS_CUTS: [&[&str]; 6] = [
    &["utime", "stime"],
    &["rmem_kb"],
    &["read_bytes"],
    &["write_bytes"],
    &["majflt"],
    &["rundelay_ns"],
];

#[test]
#[ignore = "invoked by the UI fixture build"]
fn export_real_hour_heatmaps() -> anyhow::Result<()> {
    let output = std::env::var_os("KRONIKA_REAL_HEATMAP_OUTPUT")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("KRONIKA_REAL_HEATMAP_OUTPUT is required"))?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/fixtures/real-hour.json.gz");
    let mut decoder = GzDecoder::new(BufReader::new(
        File::open(&source).with_context(|| format!("open {}", source.display()))?,
    ));
    let mut encoded = Vec::new();
    decoder.read_to_end(&mut encoded)?;
    let mut fixture: Value = serde_json::from_slice(&encoded)?;
    let from = decimal_i64(&fixture["meta"]["captureFromUs"])?;
    let hour = from - from.rem_euclid(HOUR_MICROS);
    let stored = store_process_rows(&fixture)?;
    let to = hour
        .checked_add(HOUR_MICROS)
        .and_then(|exclusive| exclusive.checked_sub(1))
        .ok_or_else(|| anyhow!("fixture hour is outside the timestamp range"))?;

    let mut heatmaps = Vec::with_capacity(PROCESS_CUTS.len() * TOPS.len());
    for fields in PROCESS_CUTS {
        for top in TOPS {
            let request = HeatmapRequest {
                from: hour,
                to,
                section: "os_process".to_owned(),
                fields: fields.iter().map(|field| (*field).to_owned()).collect(),
                columns: COLUMNS,
                top,
                group: vec!["comm".to_owned()],
                type_id: None,
            };
            let prepared = prepare(stored.path(), request)?;
            let mut records: Vec<Value> = Vec::new();
            prepared.stream(
                &mut |record| match serde_json::from_slice(&record) {
                    Ok(record) => {
                        records.push(record);
                        true
                    }
                    Err(error) => panic!("Rust Heatmap emitted invalid JSON: {error}"),
                },
                &|| false,
            )?;
            heatmaps.push(json!({
                "from": hour,
                "section": "os_process",
                "fields": fields,
                "columns": COLUMNS,
                "top": top,
                "group": ["comm"],
                "records": records,
            }));
        }
    }
    fixture
        .as_object_mut()
        .ok_or_else(|| anyhow!("real-hour fixture root is not an object"))?
        .insert("heatmaps".to_owned(), Value::Array(heatmaps));

    let mut writer = BufWriter::new(
        File::create(&output).with_context(|| format!("create {}", output.display()))?,
    );
    serde_json::to_writer(&mut writer, &fixture)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn store_process_rows(fixture: &Value) -> anyhow::Result<tempfile::TempDir> {
    let columns = fixture["os"]["columns"]
        .as_array()
        .ok_or_else(|| anyhow!("os.columns is not an array"))?;
    let positions: HashMap<&str, usize> = columns
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(|name| (name, index))
                .ok_or_else(|| anyhow!("os.columns[{index}] is not a string"))
        })
        .collect::<anyhow::Result<_>>()?;
    let snapshots = fixture["os"]["snapshots"]
        .as_array()
        .ok_or_else(|| anyhow!("os.snapshots is not an array"))?;

    let directory = tempfile::tempdir()?;
    let root = DataRoot::open(directory.path())?;
    let owner = root.acquire_writer(LayoutLimits::default())?;
    let mut journal = Journal::open(&owner, JournalConfig::default())?;
    let mut interner = Interner::new(DictLimits::default());
    let mut buffers = SectionBuffers::new();
    let mut current_segment = None;
    for snapshot in snapshots {
        if snapshot["type_id"].as_str() != Some(PROCESS_TYPE_ID) {
            continue;
        }
        let segment_id = SegmentId::new(decimal_i64(&snapshot["segment_id"])?)?;
        if current_segment.is_some_and(|current| current != segment_id) {
            finish_segment(
                &mut buffers,
                &mut interner,
                &mut journal,
                &owner,
                current_segment.expect("a changed segment has a previous value"),
            )?;
            journal.reset()?;
            buffers = SectionBuffers::new();
            interner = Interner::new(DictLimits::default());
        }
        current_segment = Some(segment_id);
        let rows = snapshot["rows"]
            .as_array()
            .ok_or_else(|| anyhow!("process snapshot rows is not an array"))?;
        for row in rows {
            let row = row
                .as_array()
                .ok_or_else(|| anyhow!("process fixture row is not an array"))?;
            let process = process_row(row, &positions, &mut interner)?;
            if let Err(process) = buffers.push(process) {
                flush_part(&mut buffers, &mut interner, &mut journal, segment_id)?;
                if buffers.push(process).is_err() {
                    bail!("one process row exceeds the registered section row bound");
                }
            }
        }
    }
    let segment_id =
        current_segment.ok_or_else(|| anyhow!("real-hour fixture has no process rows"))?;
    finish_segment(
        &mut buffers,
        &mut interner,
        &mut journal,
        &owner,
        segment_id,
    )?;
    drop(journal);
    drop(owner);
    Ok(directory)
}

fn finish_segment(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    journal: &mut Journal,
    owner: &kronika_layout::WriterOwner,
    segment_id: SegmentId,
) -> anyhow::Result<()> {
    flush_part(buffers, interner, journal, segment_id)?;
    write_segment(journal, owner, SegmentAddress::new(segment_id)?)?;
    Ok(())
}

fn flush_part(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    journal: &mut Journal,
    segment_id: SegmentId,
) -> anyhow::Result<()> {
    interner.flush_window(|window| {
        let dictionary = dict::encode(window)?;
        let part = buffers
            .flush(&dictionary)?
            .ok_or_else(|| anyhow!("cannot flush an empty process window"))?;
        journal.append(segment_id, &part)?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

fn process_row(
    row: &[Value],
    positions: &HashMap<&str, usize>,
    interner: &mut Interner,
) -> anyhow::Result<OsProcess> {
    let value = |name: &str| -> anyhow::Result<&Value> {
        let index = positions
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("real-hour fixture lacks column {name}"))?;
        row.get(index)
            .ok_or_else(|| anyhow!("real-hour fixture row lacks column {name}"))
    };
    let intern = |value: &Value, interner: &mut Interner| -> anyhow::Result<StrId> {
        let text = value
            .as_str()
            .ok_or_else(|| anyhow!("process label is not a string"))?;
        Ok(StrId(interner.intern(text.as_bytes())?.get()))
    };
    let optional_intern = |value: &Value, interner: &mut Interner| {
        if value.is_null() {
            Ok(None)
        } else {
            intern(value, interner).map(Some)
        }
    };
    Ok(OsProcess {
        ts: Ts(decimal_i64(value("ts")?)?),
        pid: decimal(value("pid")?)?,
        starttime: Ts(decimal_i64(value("starttime")?)?),
        ppid: decimal(value("ppid")?)?,
        uid: decimal(value("uid")?)?,
        euid: decimal(value("euid")?)?,
        gid: decimal(value("gid")?)?,
        egid: decimal(value("egid")?)?,
        state: decimal(value("state")?)?,
        num_threads: decimal(value("num_threads")?)?,
        tty: decimal(value("tty")?)?,
        comm: intern(value("comm")?, interner)?,
        cmdline: optional_intern(value("cmdline")?, interner)?,
        utime: decimal_i64(value("utime")?)?,
        stime: decimal_i64(value("stime")?)?,
        nice: decimal(value("nice")?)?,
        prio: decimal(value("prio")?)?,
        rtprio: decimal(value("rtprio")?)?,
        policy: decimal(value("policy")?)?,
        curcpu: decimal(value("curcpu")?)?,
        rundelay_ns: decimal_i64(value("rundelay_ns")?)?,
        blkdelay_ticks: decimal_i64(value("blkdelay_ticks")?)?,
        nvcsw: decimal_i64(value("nvcsw")?)?,
        nivcsw: decimal_i64(value("nivcsw")?)?,
        minflt: decimal_i64(value("minflt")?)?,
        majflt: decimal_i64(value("majflt")?)?,
        vmem_kb: decimal_i64(value("vmem_kb")?)?,
        rmem_kb: decimal_i64(value("rmem_kb")?)?,
        vswap_kb: decimal_i64(value("vswap_kb")?)?,
        syscr: optional_i64(value("syscr")?)?,
        syscw: optional_i64(value("syscw")?)?,
        rchar: optional_i64(value("rchar")?)?,
        wchar: optional_i64(value("wchar")?)?,
        read_bytes: optional_i64(value("read_bytes")?)?,
        write_bytes: optional_i64(value("write_bytes")?)?,
        cancelled_write_bytes: optional_i64(value("cancelled_write_bytes")?)?,
        exit_signal: decimal(value("exit_signal")?)?,
        scope: decimal(value("scope")?)?,
    })
}

fn decimal<T>(value: &Value) -> anyhow::Result<T>
where
    T: TryFrom<i64>,
    T::Error: std::fmt::Display,
{
    let number = decimal_i64(value)?;
    T::try_from(number).map_err(|error| anyhow!("integer {number} is out of range: {error}"))
}

fn decimal_i64(value: &Value) -> anyhow::Result<i64> {
    match value {
        Value::String(text) => text
            .parse()
            .with_context(|| format!("parse integer {text:?}")),
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| anyhow!("JSON number is not an i64")),
        _ => bail!("fixture cell is not an integer"),
    }
}

fn optional_i64(value: &Value) -> anyhow::Result<Option<i64>> {
    if value.is_null() {
        Ok(None)
    } else {
        decimal_i64(value).map(Some)
    }
}
