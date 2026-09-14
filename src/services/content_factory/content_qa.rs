// The judge for `bab_content`: one bab's Modul Belajar plus a sample of
// its question bank, scored against a rubric sized for what a learner
// actually experiences reading and answering it — not the whole
// 50-question bank, which would cost more to judge than to generate.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::qa::QaIssue;
use crate::services::lesson_plan::LessonPlan;

pub const DIMENSIONS: [(&str, &str); 6] = [
    ("akurasi", "fakta, rumus, dan langkah penyelesaian benar — tidak ada kekeliruan konsep"),
    ("kesesuaian_jenjang", "istilah, kedalaman, dan panjang kalimat sesuai jenjang yang dinyatakan"),
    ("kejelasan_contoh", "contoh dikerjakan bertahap dan mudah diikuti, bukan hanya hasil akhir"),
    ("cakupan", "bagian materi mencakup inti bab tanpa melebar ke bab lain dalam topik"),
    ("kualitas_soal", "sampel soal: tepat satu jawaban benar, pengecoh masuk akal, label taksonomi wajar"),
    ("kegunaan_belajar", "modul terasa seperti diajar oleh pengajar yang baik, bukan rangkuman kering"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContentScore {
    pub skor: HashMap<String, f64>,
    pub isu: Vec<QaIssue>,
}

impl ContentScore {
    pub fn average(&self) -> f64 {
        let values: Vec<f64> = DIMENSIONS.iter().filter_map(|(d, _)| self.skor.get(*d)).copied().collect();
        if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        }
    }

    pub fn minimum(&self) -> f64 {
        DIMENSIONS.iter().map(|(d, _)| self.skor.get(*d).copied().unwrap_or(0.0)).fold(f64::INFINITY, f64::min)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verdict {
    Pass,
    Repair,
}

/// Pass: average at or above the run's threshold, no dimension below 3,
/// no blocker. Same bar `qa::decide` uses for `bab_plan`.
pub fn decide(score: &ContentScore, threshold: f64) -> Verdict {
    let blocker = score.isu.iter().any(|i| i.tingkat == super::validate::Severity::Blocker);
    if !blocker && score.minimum() >= 3.0 && score.average() >= threshold {
        Verdict::Pass
    } else {
        Verdict::Repair
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairwiseContentResult {
    pub generated: ContentScore,
    pub gold: ContentScore,
    /// "generated" | "gold" | "tie"
    pub preferred: String,
    pub reason: String,
}

pub fn system_prompt() -> String {
    let rubric = DIMENSIONS.iter().map(|(d, m)| format!("- {d}: {m}")).collect::<Vec<_>>().join("\n");
    format!(
        r#"Anda adalah pengajar dan penelaah materi belajar senior yang ketat dan jujur. Anda menilai isi SATU bab: modul belajar (materi) dan sampel soal latihannya. Anda TIDAK menulis ulang; Anda menilai dan menunjukkan masalah konkret.

RUBRIK (skor 1-5 per dimensi; 5 = setara pengajar terbaik, 4 = baik dengan catatan kecil, 3 = layak tetapi jelas perlu perbaikan, 2 = bermasalah, 1 = salah/tidak layak):
{rubric}

TINGKAT ISU:
- blocker: salah fakta/konsep, jawaban kunci salah, jenjang keliru total, atau materi bab lain yang seharusnya tidak ada di sini.
- major: contoh kurang bertahap, cakupan bolong pada bagian inti, soal dengan pengecoh yang jelas salah tanpa alasan.
- minor: redaksi, variasi contoh, penyempurnaan kecil.

Nilai secara independen dan kalibrasi skor dengan jujur — jangan memberi 5 bila masih ada isu major. Keluarkan HANYA JSON tanpa teks lain."#
    )
}

/// A materi + a handful of its questions, formatted for the prompt.
/// `content` is the ALM source per section (already what the learner
/// reads); `sample_questions` is a small, representative slice of the
/// bank, not the whole thing.
pub struct ContentSample<'a> {
    pub bab_title: &'a str,
    pub level: &'a str,
    pub plan: &'a LessonPlan,
    pub sample_questions: &'a [Value],
}

fn format_sample(label: &str, s: &ContentSample) -> String {
    let sections: Vec<String> = s.plan.sections.iter().map(|sec| format!("### {}\n{}", sec.title, sec.content)).collect();
    let questions: Vec<String> = s
        .sample_questions
        .iter()
        .enumerate()
        .map(|(i, q)| format!("{}. {}", i + 1, serde_json::to_string(q).unwrap_or_default()))
        .collect();
    format!("=== {label} ===\nMATERI:\n{}\n\nSAMPEL SOAL ({} dari bank):\n{}", sections.join("\n\n"), s.sample_questions.len(), questions.join("\n"))
}

/// A real (non-benchmark) run's judge call: one bab, scored on its own,
/// no gold to compare against.
pub fn score_prompt(s: &ContentSample) -> String {
    let dims = DIMENSIONS.iter().map(|(d, _)| format!("\"{d}\":1-5")).collect::<Vec<_>>().join(",");
    format!(
        "BAB: \"{}\" · jenjang {}\n{}\n\nKeluarkan: {{\"skor\":{{{dims}}},\"isu\":[{{\"tingkat\":\"blocker|major|minor\",\"masalah\":\"…\",\"saran\":\"…\"}}]}}",
        s.bab_title,
        s.level,
        format_sample("BAB INI", s),
    )
}

fn json_slice(text: &str) -> Result<String, String> {
    let cleaned = crate::services::ai_provider::strip_code_fence(text);
    let start = cleaned.find('{').ok_or("penilai tidak mengembalikan JSON")?;
    let end = cleaned.rfind('}').ok_or("penilai tidak mengembalikan JSON")?;
    Ok(cleaned[start..=end].to_string())
}

pub fn parse_score(text: &str) -> Result<ContentScore, String> {
    let raw: RawSide = serde_json::from_str(&json_slice(text)?).map_err(|e| format!("JSON penilai tidak valid: {e}"))?;
    Ok(ContentScore { skor: clamp_scores(raw.skor), isu: raw.isu })
}

pub fn pairwise_prompt(a: &ContentSample, b: &ContentSample) -> String {
    let dims = DIMENSIONS.iter().map(|(d, _)| format!("\"{d}\":1-5")).collect::<Vec<_>>().join(",");
    format!(
        "BAB: \"{}\" · jenjang {}\nDua versi berbeda untuk bab yang sama. Nilai MASING-MASING secara independen dengan rubrik yang sama, lalu tentukan mana yang lebih baik.\n\n{}\n\n{}\n\nKeluarkan: {{\"a\":{{\"skor\":{{{dims}}},\"isu\":[{{\"tingkat\":\"blocker|major|minor\",\"masalah\":\"…\",\"saran\":\"…\"}}]}},\"b\":{{\"skor\":{{…}},\"isu\":[…]}},\"lebih_baik\":\"A|B|setara\",\"alasan\":\"…\"}}",
        a.bab_title,
        a.level,
        format_sample("VERSI A", a),
        format_sample("VERSI B", b),
    )
}

#[derive(Deserialize)]
struct RawSide {
    #[serde(default)]
    skor: HashMap<String, f64>,
    #[serde(default)]
    isu: Vec<QaIssue>,
}

#[derive(Deserialize)]
struct RawPair {
    a: RawSide,
    b: RawSide,
    #[serde(default)]
    lebih_baik: String,
    #[serde(default)]
    alasan: String,
}

fn clamp_scores(skor: HashMap<String, f64>) -> HashMap<String, f64> {
    skor.into_iter().map(|(k, v)| (k, v.clamp(1.0, 5.0))).collect()
}

/// `generated_is_a` undoes the label shuffle, same convention as
/// `benchmark.rs`'s bab_plan judge.
pub fn parse_pairwise(text: &str, generated_is_a: bool) -> Result<PairwiseContentResult, String> {
    let raw: RawPair = serde_json::from_str(&json_slice(text)?).map_err(|e| format!("JSON penilai tidak valid: {e}"))?;
    let a = ContentScore { skor: clamp_scores(raw.a.skor), isu: raw.a.isu };
    let b = ContentScore { skor: clamp_scores(raw.b.skor), isu: raw.b.isu };
    let (generated, gold) = if generated_is_a { (a, b) } else { (b, a) };
    let preferred = match (raw.lebih_baik.trim().to_uppercase().as_str(), generated_is_a) {
        ("A", true) | ("B", false) => "generated",
        ("A", false) | ("B", true) => "gold",
        _ => "tie",
    }
    .to_string();
    Ok(PairwiseContentResult { generated, gold, preferred, reason: raw.alasan })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::content_factory::validate::Severity;

    fn score(v: f64, blocker: bool) -> ContentScore {
        ContentScore {
            skor: DIMENSIONS.iter().map(|(d, _)| (d.to_string(), v)).collect(),
            isu: if blocker { vec![QaIssue { tingkat: Severity::Blocker, bab: None, masalah: "salah konsep".into(), saran: "".into() }] } else { vec![] },
        }
    }

    #[test]
    fn pairwise_labels_are_unshuffled() {
        let text = r#"{"a":{"skor":{"akurasi":5}},"b":{"skor":{"akurasi":3}},"lebih_baik":"A","alasan":"lebih akurat"}"#;
        let as_b = parse_pairwise(text, false).unwrap();
        assert_eq!(as_b.preferred, "gold", "A was the gold side");
        assert_eq!(as_b.gold.skor["akurasi"], 5.0);
        let as_a = parse_pairwise(text, true).unwrap();
        assert_eq!(as_a.preferred, "generated");
    }

    #[test]
    fn average_ignores_a_blocker_it_does_not_score_a_dimension_for() {
        let s = score(4.5, true);
        assert!((s.average() - 4.5).abs() < 1e-9);
        assert_eq!(s.isu.len(), 1);
    }

    #[test]
    fn decide_needs_the_average_the_floor_and_no_blocker() {
        assert_eq!(decide(&score(4.5, false), 4.2), Verdict::Pass);
        assert_eq!(decide(&score(4.0, false), 4.2), Verdict::Repair, "under the threshold");
        assert_eq!(decide(&score(5.0, true), 4.2), Verdict::Repair, "a blocker fails a perfect score");
    }

    #[test]
    fn parse_score_reads_a_single_side_reply() {
        let text = r#"```json
{"skor":{"akurasi":4,"kualitas_soal":5},"isu":[{"tingkat":"minor","masalah":"redaksi kaku","saran":""}]}
```"#;
        let s = parse_score(text).unwrap();
        assert_eq!(s.skor["akurasi"], 4.0);
        assert_eq!(s.isu.len(), 1);
    }
}
