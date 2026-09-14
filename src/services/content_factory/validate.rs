// Rule checks on a bab plan — no AI, no tokens, run on every draft
// before the judge sees it. Anything a rule can catch is caught here, so
// the (expensive, stronger) judge model spends its attention on what
// only judgement can see: standards, sequencing, overlap in meaning.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::plan::{learner_word, TopicContext, TopicDraft, JENIS_SOAL};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Minor,
    Major,
    Blocker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub topic_id: Uuid,
    pub severity: Severity,
    pub code: String,
    /// Which bab (1-based), when the issue is about one.
    pub bab: Option<usize>,
    pub message: String,
}

const GENERIC_TITLES: [&str; 12] = ["pengantar", "pendahuluan", "rangkuman", "ringkasan", "kesimpulan", "latihan", "latihan soal", "evaluasi", "penutup", "review", "materi", "pembahasan"];
/// Verbs that name a state of mind, not something a learner can be seen
/// doing — the classic non-operational objective.
const VAGUE_VERBS: [&str; 5] = ["memahami", "mengetahui", "mengerti", "menguasai", "mengenal konsep"];
const MAX_TITLE_CHARS: usize = 90;

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

const STOPWORDS: [&str; 14] = ["dan", "yang", "di", "ke", "dari", "pada", "untuk", "dengan", "dalam", "serta", "atau", "the", "of", "and"];

fn words(s: &str) -> HashSet<String> {
    normalize(s).split(' ').filter(|w| w.len() > 2 && !STOPWORDS.contains(w)).map(str::to_string).collect()
}

pub fn jaccard(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / a.union(&b).count() as f64
}

/// Checks the drafts for `targets` (the topics that were asked for).
/// `jenjang` decides the expected subject of each `tujuan`.
pub fn validate(targets: &[&TopicContext], drafts: &[TopicDraft], jenjang: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    let learner = learner_word(jenjang);
    let mut push = |topic_id: Uuid, severity: Severity, code: &str, bab: Option<usize>, message: String| issues.push(Issue { topic_id, severity, code: code.into(), bab, message });

    // All bab titles of the domain, for cross-topic overlap.
    let all_titles: Vec<(Uuid, usize, &str)> = drafts.iter().flat_map(|d| d.bab.iter().enumerate().map(move |(i, b)| (d.topic_id, i, b.judul.as_str()))).collect();

    for topic in targets {
        let Some(draft) = drafts.iter().find(|d| d.topic_id == topic.topic_id) else {
            push(topic.topic_id, Severity::Blocker, "missing_topic", None, format!("topik \"{}\" tidak ada di keluaran", topic.title));
            continue;
        };
        if normalize(&draft.t) != normalize(&topic.title) {
            push(topic.topic_id, Severity::Blocker, "topic_mismatch", None, format!("judul topik di keluaran \"{}\" tidak sama dengan \"{}\"", draft.t, topic.title));
        }
        let n = draft.bab.len();
        if !(3..=8).contains(&n) {
            push(topic.topic_id, Severity::Blocker, "bab_count", None, format!("{n} bab; harus 3–8 (umumnya 4–6)"));
        } else if !(4..=6).contains(&n) {
            push(topic.topic_id, Severity::Minor, "bab_count_unusual", None, format!("{n} bab; pastikan memang topiknya sesempit/seluas itu"));
        }
        if let Some(j) = &draft.jenis_soal {
            if !JENIS_SOAL.contains(&j.as_str()) {
                push(topic.topic_id, Severity::Minor, "jenis_soal", None, format!("jenis_soal \"{j}\" tidak dikenal"));
            }
        }

        let mut seen_cakupan: HashSet<String> = HashSet::new();
        for (i, bab) in draft.bab.iter().enumerate() {
            let no = Some(i + 1);
            if bab.judul.is_empty() || bab.tujuan.is_empty() {
                push(topic.topic_id, Severity::Blocker, "empty_field", no, "judul atau tujuan kosong".into());
                continue;
            }
            if bab.judul.chars().count() > MAX_TITLE_CHARS {
                push(topic.topic_id, Severity::Major, "title_too_long", no, format!("judul {} karakter (maks {MAX_TITLE_CHARS})", bab.judul.chars().count()));
            }
            if GENERIC_TITLES.contains(&normalize(&bab.judul).as_str()) {
                push(topic.topic_id, Severity::Major, "generic_title", no, format!("judul \"{}\" terlalu umum", bab.judul));
            }
            if normalize(&bab.judul) == normalize(&topic.title) {
                push(topic.topic_id, Severity::Major, "title_equals_topic", no, "judul bab sama dengan judul topik".into());
            }
            if bab.cakupan.len() < 2 {
                push(topic.topic_id, Severity::Major, "cakupan_thin", no, format!("cakupan hanya {} butir (minimal 3)", bab.cakupan.len()));
            } else if bab.cakupan.len() < 3 || bab.cakupan.len() > 6 {
                push(topic.topic_id, Severity::Minor, "cakupan_count", no, format!("cakupan {} butir (umumnya 3–4)", bab.cakupan.len()));
            }
            for c in &bab.cakupan {
                if !seen_cakupan.insert(normalize(c)) {
                    push(topic.topic_id, Severity::Minor, "cakupan_repeated", no, format!("cakupan \"{c}\" sudah muncul di bab lain"));
                }
            }
            if !bab.tujuan.starts_with(learner) {
                push(topic.topic_id, Severity::Minor, "tujuan_subject", no, format!("tujuan sebaiknya diawali \"{learner}\" untuk jenjang ini"));
            }
            let lower = bab.tujuan.to_lowercase();
            if let Some(verb) = VAGUE_VERBS.iter().find(|v| lower.contains(&format!("dapat {v}")) || lower.starts_with(&format!("{} {v}", learner.to_lowercase()))) {
                push(topic.topic_id, Severity::Minor, "tujuan_vague", no, format!("tujuan memakai \"{verb}\" — gunakan kata kerja operasional"));
            }
            // Same bab twice, inside the topic or across the domain.
            for &(other_topic, j, other) in &all_titles {
                let same_slot = other_topic == topic.topic_id && j == i;
                let earlier_in_topic = other_topic == topic.topic_id && j < i;
                let other_topic_only = other_topic != topic.topic_id && topic.topic_id < other_topic;
                if same_slot || !(earlier_in_topic || other_topic_only) {
                    continue;
                }
                if normalize(other) == normalize(&bab.judul) || jaccard(other, &bab.judul) >= 0.6 {
                    let (code, where_) = if other_topic == topic.topic_id { ("duplicate_bab", "bab lain di topik ini") } else { ("duplicate_bab_domain", "bab di topik lain dalam domain") };
                    push(topic.topic_id, Severity::Major, code, no, format!("judul \"{}\" terlalu mirip dengan {where_}: \"{other}\"", bab.judul));
                }
            }
        }
    }
    issues
}

/// Topics with a blocker or major issue need another generation round
/// before the judge is worth asking.
pub fn needs_repair(issues: &[Issue]) -> HashSet<Uuid> {
    issues.iter().filter(|i| i.severity >= Severity::Major).map(|i| i.topic_id).collect()
}

#[cfg(test)]
mod tests {
    use super::super::plan::{BabDraft, TopicStatus};
    use super::*;

    fn ctx(title: &str) -> TopicContext {
        TopicContext { topic_id: Uuid::new_v4(), title: title.into(), paths: vec![], existing_babs: vec![] }
    }

    fn bab(judul: &str) -> BabDraft {
        BabDraft { judul: judul.into(), tujuan: "Siswa dapat menghitung besar resultan dua vektor.".into(), cakupan: vec![format!("{judul} satu"), format!("{judul} dua"), format!("{judul} tiga")] }
    }

    fn draft(topic: &TopicContext, babs: Vec<BabDraft>) -> TopicDraft {
        TopicDraft { topic_id: topic.topic_id, t: topic.title.clone(), jenis_soal: None, bab: babs, status: TopicStatus::Pending }
    }

    #[test]
    fn a_clean_plan_has_no_issues() {
        let t = ctx("Penjumlahan Vektor");
        let d = draft(&t, vec![bab("Resultan Vektor Segaris"), bab("Metode Grafis Poligon"), bab("Rumus Kosinus Jajargenjang"), bab("Arah Resultan dan Selisih")]);
        assert_eq!(validate(&[&t], &[d], "SMA/MA kelas 10"), vec![]);
    }

    #[test]
    fn each_rule_fires() {
        let t = ctx("Penjumlahan Vektor");
        let mut babs = vec![bab("Pengantar"), bab("Penjumlahan Vektor"), bab("Metode Grafis Poligon Vektor"), bab("Metode Grafis Poligon Vektor Lanjut")];
        babs[0].tujuan = "Mahasiswa dapat memahami vektor.".into();
        babs[1].cakupan = vec!["x".into()];
        let d = draft(&t, babs);
        let codes: HashSet<String> = validate(&[&t], &[d], "SMA/MA").into_iter().map(|i| i.code).collect();
        for code in ["generic_title", "title_equals_topic", "cakupan_thin", "tujuan_subject", "tujuan_vague", "duplicate_bab"] {
            assert!(codes.contains(code), "expected {code} in {codes:?}");
        }
    }

    #[test]
    fn missing_topics_and_wrong_counts_are_blockers() {
        let a = ctx("A topik");
        let b = ctx("B topik");
        let issues = validate(&[&a, &b], &[draft(&a, vec![bab("Satu-satunya bab")])], "SMP");
        assert!(issues.iter().any(|i| i.topic_id == b.topic_id && i.code == "missing_topic" && i.severity == Severity::Blocker));
        assert!(issues.iter().any(|i| i.topic_id == a.topic_id && i.code == "bab_count" && i.severity == Severity::Blocker));
        assert_eq!(needs_repair(&issues).len(), 2);
    }

    #[test]
    fn duplicates_across_topics_are_reported_once() {
        let a = ctx("Hukum I Newton");
        let b = ctx("Hukum II Newton");
        let shared = "Diagram Benda Bebas pada Bidang Miring";
        let da = draft(&a, vec![bab(shared), bab("Inersia dan Massa"), bab("Kesetimbangan Partikel"), bab("Kerangka Acuan Inersial")]);
        let db = draft(&b, vec![bab(shared), bab("Gaya dan Percepatan"), bab("Berat Semu di Lift"), bab("Sistem Dua Benda")]);
        let dup: Vec<Issue> = validate(&[&a, &b], &[da, db], "SMA").into_iter().filter(|i| i.code == "duplicate_bab_domain").collect();
        assert_eq!(dup.len(), 1, "{dup:?}");
    }
}
