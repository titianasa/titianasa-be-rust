// The judge: a stronger model (role `agent_qa`) scoring what the
// generator wrote against a fixed rubric. Two modes — scoring one plan
// (every normal task), and scoring two plans blind side by side (the
// benchmark: Gemini against the gold plan, labels shuffled).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::plan::{context_block, BabDraft, DomainContext, TopicContext, TopicDraft};
use super::validate::Severity;

pub const DIMENSIONS: [(&str, &str); 7] = [
    ("kesesuaian_standar", "kedalaman, istilah, dan cakupan sesuai jenjang dan acuan standar"),
    ("urutan", "urutan bab mengikuti prasyarat dan alur belajar yang wajar"),
    ("granularitas", "setiap bab ≈ satu sesi 45 menit, tidak terlalu sempit atau terlalu luas"),
    ("tanpa_tumpang_tindih", "tidak ada materi yang diulang antarbab, antartopik, atau dengan domain tetangga"),
    ("kelengkapan", "inti topik tercakup lengkap, tidak ada konsep penting yang hilang"),
    ("kegunaan_jalur", "berguna bagi semua learning path yang memakai topik (kurikulum nasional, internasional, ujian)"),
    ("kejelasan_tujuan", "tujuan terukur dengan kata kerja operasional, cakupan konkret"),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QaIssue {
    pub tingkat: Severity,
    #[serde(default)]
    pub bab: Option<usize>,
    pub masalah: String,
    #[serde(default)]
    pub saran: String,
}

/// The judge is asked for `{"tingkat","bab","masalah","saran"}` per issue,
/// but a model — gemini-3.1-pro-preview included — sometimes collapses
/// one into a plain string like `"minor: Bab 'X' idealnya..."` instead.
/// Accepting that shape here (parsed back into the same fields) is
/// cheaper than a repair round over a judge output that is otherwise
/// perfectly usable.
impl<'de> Deserialize<'de> for QaIssue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Object {
                tingkat: Severity,
                #[serde(default)]
                bab: Option<usize>,
                masalah: String,
                #[serde(default)]
                saran: String,
            },
            Text(String),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Object { tingkat, bab, masalah, saran } => QaIssue { tingkat, bab, masalah, saran },
            Raw::Text(s) => {
                let lower = s.to_lowercase();
                // Fixed ASCII prefixes, so slicing `s` by the prefix's own
                // byte length (not `rest`'s) is always a char boundary.
                let (tingkat, rest) = if lower.starts_with("blocker:") {
                    (Severity::Blocker, &s["blocker:".len()..])
                } else if lower.starts_with("major:") {
                    (Severity::Major, &s["major:".len()..])
                } else if lower.starts_with("minor:") {
                    (Severity::Minor, &s["minor:".len()..])
                } else {
                    (Severity::Minor, s.as_str())
                };
                QaIssue { tingkat, bab: None, masalah: rest.trim().to_string(), saran: String::new() }
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopicScore {
    pub topic_id: Uuid,
    pub skor: HashMap<String, f64>,
    pub isu: Vec<QaIssue>,
}

impl TopicScore {
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
/// no blocker. Anything else is sent back for repair (or to review once
/// repairs are spent).
pub fn decide(score: &TopicScore, threshold: f64) -> Verdict {
    let blocker = score.isu.iter().any(|i| i.tingkat == Severity::Blocker);
    if !blocker && score.minimum() >= 3.0 && score.average() >= threshold {
        Verdict::Pass
    } else {
        Verdict::Repair
    }
}

/// The problems to hand the generator for a repair round: blockers and
/// majors, plus low-scoring dimensions named as such.
pub fn repair_notes(score: &TopicScore, threshold: f64) -> Vec<String> {
    let mut notes: Vec<String> = score
        .isu
        .iter()
        .filter(|i| i.tingkat >= Severity::Major)
        .map(|i| {
            let where_ = i.bab.map(|b| format!("bab {b}: ")).unwrap_or_default();
            if i.saran.is_empty() { format!("{where_}{}", i.masalah) } else { format!("{where_}{} → {}", i.masalah, i.saran) }
        })
        .collect();
    for (dim, meaning) in DIMENSIONS {
        if let Some(v) = score.skor.get(dim) {
            if *v < threshold.min(4.0) {
                notes.push(format!("skor {dim} hanya {v}: perbaiki agar {meaning}"));
            }
        }
    }
    notes
}

pub fn system_prompt() -> String {
    let rubric = DIMENSIONS.iter().map(|(d, m)| format!("- {d}: {m}")).collect::<Vec<_>>().join("\n");
    format!(
        r#"Anda adalah penelaah kurikulum senior yang ketat dan jujur. Anda menilai rencana BAB (judul, tujuan, cakupan) per topik untuk platform belajar. Anda TIDAK menulis ulang rencana; Anda menilai dan menunjukkan masalah konkret.

RUBRIK (skor 1–5 per dimensi; 5 = setara penyusun kurikulum terbaik, 4 = baik dengan catatan kecil, 3 = layak tetapi jelas perlu perbaikan, 2 = bermasalah, 1 = salah/tidak layak):
{rubric}

TINGKAT ISU:
- blocker: salah fakta/konsep, tidak sesuai jenjang secara nyata, topik keliru, bab penting hilang, atau tumpang tindih besar.
- major: urutan keliru, bab terlalu luas/sempit, tujuan tidak terukur, cakupan kabur, tumpang tindih sebagian.
- minor: redaksi, istilah kurang tepat, penyempurnaan kecil.

Nilai secara independen dan kalibrasi skor dengan jujur — jangan memberi 5 bila masih ada isu major. Keluarkan HANYA JSON tanpa teks lain."#
    )
}

fn plan_json(topics: &[(&TopicContext, &[BabDraft])]) -> String {
    let list: Vec<serde_json::Value> = topics
        .iter()
        .enumerate()
        .map(|(i, (t, bab))| serde_json::json!({"no": i + 1, "topik": t.title, "dipakai_di": t.paths, "bab": bab}))
        .collect();
    serde_json::to_string(&list).unwrap_or_default()
}

pub fn score_prompt(ctx: &DomainContext, drafts: &[&TopicDraft]) -> (String, Vec<Uuid>) {
    let topics: Vec<(&TopicContext, &[BabDraft])> = drafts
        .iter()
        .filter_map(|d| ctx.topics.iter().find(|t| t.topic_id == d.topic_id).map(|t| (t, d.bab.as_slice())))
        .collect();
    let order = topics.iter().map(|(t, _)| t.topic_id).collect();
    let dims = DIMENSIONS.iter().map(|(d, _)| format!("\"{d}\":1-5")).collect::<Vec<_>>().join(",");
    let prompt = format!(
        "{}\nRENCANA YANG DINILAI:\n{}\n\nKeluarkan: {{\"topik\":[{{\"no\":1,\"skor\":{{{dims}}},\"isu\":[{{\"tingkat\":\"blocker|major|minor\",\"bab\":2,\"masalah\":\"…\",\"saran\":\"…\"}}]}}]}}",
        context_block(ctx),
        plan_json(&topics)
    );
    (prompt, order)
}

/// Blind side-by-side: `a` and `b` are shown as "Rencana A/B"; the caller
/// decides (and records) which one is the generator's.
pub fn pairwise_prompt(ctx: &DomainContext, topics: &[&TopicContext], a: &[&[BabDraft]], b: &[&[BabDraft]]) -> String {
    let side = |plans: &[&[BabDraft]]| plan_json(&topics.iter().zip(plans.iter()).map(|(t, p)| (*t, *p)).collect::<Vec<_>>());
    let dims = DIMENSIONS.iter().map(|(d, _)| format!("\"{d}\":1-5")).collect::<Vec<_>>().join(",");
    format!(
        "{}\nDua rencana berbeda untuk topik yang sama. Nilai MASING-MASING secara independen dengan rubrik yang sama, lalu tentukan mana yang lebih baik per topik.\n\nRENCANA A:\n{}\n\nRENCANA B:\n{}\n\nKeluarkan: {{\"topik\":[{{\"no\":1,\"a\":{{\"skor\":{{{dims}}},\"isu\":[…]}},\"b\":{{\"skor\":{{…}},\"isu\":[…]}},\"lebih_baik\":\"A|B|setara\",\"alasan\":\"…\"}}]}}",
        context_block(ctx),
        side(a),
        side(b)
    )
}

#[derive(Deserialize)]
struct RawScored {
    topik: Vec<RawScoredTopic>,
}

#[derive(Deserialize)]
struct RawScoredTopic {
    no: usize,
    #[serde(default)]
    skor: HashMap<String, f64>,
    #[serde(default)]
    isu: Vec<QaIssue>,
}

fn json_slice(text: &str) -> Result<String, String> {
    let cleaned = crate::services::ai_provider::strip_code_fence(text);
    let start = cleaned.find('{').ok_or("penilai tidak mengembalikan JSON")?;
    let end = cleaned.rfind('}').ok_or("penilai tidak mengembalikan JSON")?;
    Ok(cleaned[start..=end].to_string())
}

fn clamp_scores(skor: HashMap<String, f64>) -> HashMap<String, f64> {
    skor.into_iter().map(|(k, v)| (k, v.clamp(1.0, 5.0))).collect()
}

pub fn parse_scores(text: &str, order: &[Uuid]) -> Result<Vec<TopicScore>, String> {
    let raw: RawScored = serde_json::from_str(&json_slice(text)?).map_err(|e| format!("JSON penilai tidak valid: {e}"))?;
    Ok(raw
        .topik
        .into_iter()
        .filter_map(|t| order.get(t.no.checked_sub(1)?).map(|id| TopicScore { topic_id: *id, skor: clamp_scores(t.skor), isu: t.isu }))
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairwiseResult {
    pub topic_id: Uuid,
    pub generated: TopicScore,
    pub gold: TopicScore,
    /// "generated" | "gold" | "tie"
    pub preferred: String,
    pub reason: String,
}

#[derive(Deserialize)]
struct RawPair {
    topik: Vec<RawPairTopic>,
}

#[derive(Deserialize)]
struct RawSide {
    #[serde(default)]
    skor: HashMap<String, f64>,
    #[serde(default)]
    isu: Vec<QaIssue>,
}

#[derive(Deserialize)]
struct RawPairTopic {
    no: usize,
    a: RawSide,
    b: RawSide,
    #[serde(default)]
    lebih_baik: String,
    #[serde(default)]
    alasan: String,
}

/// `generated_is_a` undoes the label shuffle.
pub fn parse_pairwise(text: &str, order: &[Uuid], generated_is_a: bool) -> Result<Vec<PairwiseResult>, String> {
    let raw: RawPair = serde_json::from_str(&json_slice(text)?).map_err(|e| format!("JSON penilai tidak valid: {e}"))?;
    Ok(raw
        .topik
        .into_iter()
        .filter_map(|t| {
            let id = *order.get(t.no.checked_sub(1)?)?;
            let a = TopicScore { topic_id: id, skor: clamp_scores(t.a.skor), isu: t.a.isu };
            let b = TopicScore { topic_id: id, skor: clamp_scores(t.b.skor), isu: t.b.isu };
            let (generated, gold) = if generated_is_a { (a, b) } else { (b, a) };
            let preferred = match (t.lebih_baik.trim().to_uppercase().as_str(), generated_is_a) {
                ("A", true) | ("B", false) => "generated",
                ("A", false) | ("B", true) => "gold",
                _ => "tie",
            }
            .to_string();
            Some(PairwiseResult { topic_id: id, generated, gold, preferred, reason: t.alasan })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(values: [f64; 7], blocker: bool) -> TopicScore {
        TopicScore {
            topic_id: Uuid::nil(),
            skor: DIMENSIONS.iter().zip(values).map(|((d, _), v)| (d.to_string(), v)).collect(),
            isu: if blocker { vec![QaIssue { tingkat: Severity::Blocker, bab: Some(2), masalah: "konsep keliru".into(), saran: "betulkan".into() }] } else { vec![] },
        }
    }

    #[test]
    fn decide_needs_the_average_the_floor_and_no_blocker() {
        assert_eq!(decide(&score([5.0, 4.0, 4.0, 4.0, 4.0, 5.0, 4.0], false), 4.2), Verdict::Pass);
        assert_eq!(decide(&score([5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 2.0], false), 4.2), Verdict::Repair, "one dimension under 3 fails even with a high average");
        assert_eq!(decide(&score([4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0], false), 4.2), Verdict::Repair, "under the threshold");
        assert_eq!(decide(&score([5.0; 7], true), 4.2), Verdict::Repair, "a blocker fails a perfect score");
    }

    #[test]
    fn repair_notes_name_blockers_and_weak_dimensions() {
        let notes = repair_notes(&score([5.0, 3.0, 5.0, 5.0, 5.0, 5.0, 5.0], true), 4.2);
        assert!(notes.iter().any(|n| n.starts_with("bab 2: konsep keliru")));
        assert!(notes.iter().any(|n| n.contains("urutan")));
    }

    #[test]
    fn an_issue_written_as_a_plain_string_still_parses() {
        // Seen from gemini-3.1-pro-preview in practice: one issue in the
        // array came back as a bare string instead of the requested
        // {tingkat, bab, masalah, saran} object.
        let order = vec![Uuid::new_v4()];
        let text = r#"{"topik":[{"no":1,"skor":{"urutan":4},"isu":["minor: Bab 'Replikasi' idealnya dibahas lebih awal.",{"tingkat":"major","bab":2,"masalah":"tumpang tindih","saran":"gabungkan"}]}]}"#;
        let scores = parse_scores(text, &order).unwrap();
        assert_eq!(scores[0].isu.len(), 2);
        assert_eq!(scores[0].isu[0].tingkat, Severity::Minor);
        assert_eq!(scores[0].isu[0].masalah, "Bab 'Replikasi' idealnya dibahas lebih awal.");
        assert_eq!(scores[0].isu[0].bab, None);
        assert_eq!(scores[0].isu[1].tingkat, Severity::Major);
        assert_eq!(scores[0].isu[1].bab, Some(2));
    }

    #[test]
    fn an_issue_string_with_no_recognized_prefix_falls_back_to_minor() {
        let order = vec![Uuid::new_v4()];
        let text = r#"{"topik":[{"no":1,"skor":{},"isu":["cakupan bab 3 agak tipis"]}]}"#;
        let scores = parse_scores(text, &order).unwrap();
        assert_eq!(scores[0].isu[0].tingkat, Severity::Minor);
        assert_eq!(scores[0].isu[0].masalah, "cakupan bab 3 agak tipis");
    }

    #[test]
    fn pairwise_labels_are_unshuffled() {
        let id = Uuid::new_v4();
        let text = r#"{"topik":[{"no":1,"a":{"skor":{"urutan":5}},"b":{"skor":{"urutan":3}},"lebih_baik":"A","alasan":"lebih runtut"}]}"#;
        let as_b = parse_pairwise(text, &[id], false).unwrap();
        assert_eq!(as_b[0].preferred, "gold", "A was the gold plan");
        assert_eq!(as_b[0].gold.skor["urutan"], 5.0);
        let as_a = parse_pairwise(text, &[id], true).unwrap();
        assert_eq!(as_a[0].preferred, "generated");
    }
}
