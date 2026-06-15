use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("cli crate should live two levels below repo root")
        .to_path_buf()
}

fn make_temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "agentflow-local-survival-assoc-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

fn run_script(dir: &Path, gene: &str, expression: &str, survival: &str) -> Output {
    let expression_path = dir.join("expression.tsv");
    let survival_path = dir.join("survival.tsv");
    let report_path = dir.join("report.md");
    fs::write(&expression_path, expression).expect("expression fixture should be written");
    fs::write(&survival_path, survival).expect("survival fixture should be written");

    Command::new("/usr/bin/env")
        .arg("python3")
        .arg(repo_root().join("examples/tools/local_survival_assoc.py"))
        .env("AGENTFLOW_INPUT_EXPRESSION_TABLE", &expression_path)
        .env("AGENTFLOW_INPUT_SURVIVAL_TABLE", &survival_path)
        .env("AGENTFLOW_PARAM_GENE", gene)
        .env("AGENTFLOW_OUTPUT_REPORT", &report_path)
        .output()
        .expect("python3 should run local_survival_assoc.py")
}

fn read_report(dir: &Path) -> String {
    fs::read_to_string(dir.join("report.md")).expect("report should be written")
}

#[test]
fn local_survival_assoc_reports_worse_direction_for_high_expression_signal() {
    let dir = make_temp_dir("signal");
    let expression = "\
sample\tGENE1\tNOISE
s1\t9.0\t1
s2\t8.5\t1
s3\t8.0\t1
s4\t7.5\t1
s5\t2.0\t1
s6\t1.8\t1
s7\t1.5\t1
s8\t1.2\t1
";
    let survival = "\
sample\ttime\tstatus
s1\t4\t1
s2\t5\t1
s3\t6\t1
s4\t7\t1
s5\t30\t0
s6\t32\t0
s7\t34\t0
s8\t36\t0
";

    let output = run_script(&dir, "GENE1", expression, survival);

    assert!(
        output.status.success(),
        "script should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = read_report(&dir);
    assert!(report.contains("Gene: GENE1"));
    assert!(report.contains("logrank_p:"));
    assert!(report.contains("direction: high-expression associated with worse overall survival"));
}

#[test]
fn local_survival_assoc_exits_nonzero_when_gene_column_is_missing() {
    let dir = make_temp_dir("missing-gene");
    let expression = "\
sample\tGENE1
s1\t9
s2\t8
s3\t7
s4\t6
s5\t1
s6\t2
";
    let survival = "\
sample\ttime\tstatus
s1\t4\t1
s2\t5\t1
s3\t6\t1
s4\t30\t0
s5\t31\t0
s6\t32\t0
";

    let output = run_script(&dir, "MISSING", expression, survival);

    assert!(
        !output.status.success(),
        "script should reject a gene absent from the expression table"
    );
}

#[test]
fn local_survival_assoc_exits_nonzero_when_join_has_too_few_samples() {
    let dir = make_temp_dir("too-few");
    let expression = "\
sample\tGENE1
s1\t9
s2\t8
s3\t7
s4\t1
s5\t2
s6\t3
";
    let survival = "\
sample\ttime\tstatus
s1\t4\t1
s2\t5\t1
s3\t6\t1
s4\t30\t0
s9\t31\t0
s10\t32\t0
";

    let output = run_script(&dir, "GENE1", expression, survival);

    assert!(
        !output.status.success(),
        "script should reject joined cohorts with fewer than six samples"
    );
}
