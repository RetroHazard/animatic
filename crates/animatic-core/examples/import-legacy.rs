use std::path::PathBuf;

use animatic_core::db::Db;
use animatic_core::legacy::import::import_databank;

fn main() -> animatic_core::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().ok_or_else(|| {
        animatic_core::Error::Other("usage: import-legacy <AOM/kowai> <output.sqlite3>".into())
    })?);
    let output = PathBuf::from(args.next().ok_or_else(|| {
        animatic_core::Error::Other("usage: import-legacy <AOM/kowai> <output.sqlite3>".into())
    })?);
    let temp = animatic_core::legacy::workdir::default_temp_root();
    let mut db = Db::open(&output)?;
    let report = import_databank(&mut db, &source, &temp, &mut |stage, fraction| {
        eprintln!("{:>6.1}%  {stage}", fraction * 100.0);
        true
    })?;
    let records: usize = report.imported.iter().map(|(_, n)| *n).sum();
    println!(
        "Imported {records} records from {} collections into {}",
        report.imported.len(),
        output.display()
    );
    Ok(())
}
