// `bab_plan` — the first Pabrik Konten kind: designing the babs of every
// topic in one domain. What the model is given (context), what it is
// asked (prompt), and how its answer is read back (parse).
//
// A domain, not a topic, is the unit on purpose: the babs of "Hukum I
// Newton" and "Hukum II Newton" are only non-overlapping if they are
// designed looking at each other.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

pub const PROMPT_VERSION: &str = "bab_plan.v1";
pub const JENIS_SOAL: [&str; 3] = ["hitungan", "konsep", "bahasa"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BabDraft {
    pub judul: String,
    pub tujuan: String,
    pub cakupan: Vec<String>,
}

/// One topic's plan. `status` is the factory's own bookkeeping per topic
/// inside a domain task: pending → passed/failed → applied/needs_review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopicDraft {
    pub topic_id: Uuid,
    pub t: String,
    #[serde(default)]
    pub jenis_soal: Option<String>,
    pub bab: Vec<BabDraft>,
    #[serde(default)]
    pub status: TopicStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TopicStatus {
    #[default]
    Pending,
    Passed,
    Applied,
    NeedsReview,
    Skipped,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Standard {
    pub jenjang: String,
    pub standar: Vec<String>,
    pub bahasa: String,
    pub jenis_soal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicContext {
    pub topic_id: Uuid,
    pub title: String,
    /// Learning paths that reference this topic, trimmed to 3 segments.
    pub paths: Vec<String>,
    /// Babs it already has (a topic with babs is not re-planned in v1).
    pub existing_babs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighbourDomain {
    pub title: String,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainContext {
    pub domain_folder_id: Uuid,
    pub domain_title: String,
    pub tahap_folder_id: Uuid,
    pub tahap_title: String,
    pub subject_folder_title: String,
    pub subject_id: Option<Uuid>,
    pub standard: Standard,
    /// Every topic of the domain (for boundaries) …
    pub topics: Vec<TopicContext>,
    /// … of which these are the ones to plan.
    pub target_topic_ids: Vec<Uuid>,
    pub previous_domain: Option<NeighbourDomain>,
    pub next_domain: Option<NeighbourDomain>,
}

impl DomainContext {
    pub fn targets(&self) -> Vec<&TopicContext> {
        self.topics.iter().filter(|t| self.target_topic_ids.contains(&t.topic_id)).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exemplar {
    pub title: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
}

/// Everything about a domain the generator and the judge need, read
/// straight from the library tree (Semua Mata Pelajaran › mapel › tahap
/// › domain › topik).
pub async fn load_context(pool: &PgPool, domain_folder_id: Uuid, target_topic_ids: Option<&[Uuid]>) -> Result<DomainContext, AppError> {
    let domain = sqlx::query!(
        r#"select d.id, d.title, d.parent_id as "tahap_id!", t.title as tahap_title, t.parent_id as "subject_folder_id!", s.title as subject_folder_title
           from modules d join modules t on t.id = d.parent_id join modules s on s.id = t.parent_id
           where d.id = $1 and d.is_folder"#,
        domain_folder_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("domain_not_found"))?;

    let standard = sqlx::query!(r#"select jenjang, standar, bahasa, jenis_soal from curriculum_standards where tahap_folder_id = $1"#, domain.tahap_id)
        .fetch_optional(pool)
        .await?
        .map(|r| Standard { jenjang: r.jenjang, standar: r.standar, bahasa: r.bahasa, jenis_soal: r.jenis_soal })
        .ok_or_else(|| AppError::UnprocessableEntity("standard_missing", format!("tahap \"{}\" belum punya standar kurikulum", domain.tahap_title)))?;

    let topic_rows = sqlx::query!(
        r#"select m.id, m.title, m.subject_id,
                  coalesce((select array_agg(i.title order by i.order_index) from module_items i where i.module_id = m.id and i.node_type = 'section'), '{}') as "existing!"
           from modules m
           where m.parent_id = $1 and not m.is_folder and m.source_module_id is null
           order by m.order_index, m.title"#,
        domain_folder_id,
    )
    .fetch_all(pool)
    .await?;
    let topic_ids: Vec<Uuid> = topic_rows.iter().map(|r| r.id).collect();
    let paths = learning_paths(pool, &topic_ids).await?;
    let subject_id = topic_rows.iter().find_map(|r| r.subject_id);
    let topics: Vec<TopicContext> = topic_rows
        .into_iter()
        .map(|r| TopicContext { topic_id: r.id, title: r.title, paths: paths.get(&r.id).cloned().unwrap_or_default(), existing_babs: r.existing })
        .collect();

    let target_topic_ids = match target_topic_ids {
        Some(ids) => topics.iter().filter(|t| ids.contains(&t.topic_id)).map(|t| t.topic_id).collect(),
        None => topics.iter().filter(|t| t.existing_babs.is_empty()).map(|t| t.topic_id).collect(),
    };

    let siblings = sqlx::query!(
        r#"select d.id, d.title,
                  coalesce((select array_agg(m.title order by m.order_index) from modules m where m.parent_id = d.id and not m.is_folder), '{}') as "topics!"
           from modules d where d.parent_id = $1 and d.is_folder order by d.order_index, d.title"#,
        domain.tahap_id,
    )
    .fetch_all(pool)
    .await?;
    let position = siblings.iter().position(|s| s.id == domain_folder_id);
    let neighbour = |i: Option<usize>| i.and_then(|i| siblings.get(i)).map(|s| NeighbourDomain { title: s.title.clone(), topics: s.topics.clone() });

    Ok(DomainContext {
        domain_folder_id,
        domain_title: domain.title,
        tahap_folder_id: domain.tahap_id,
        tahap_title: domain.tahap_title,
        subject_folder_title: domain.subject_folder_title,
        subject_id,
        standard,
        topics,
        target_topic_ids,
        previous_domain: neighbour(position.and_then(|p| p.checked_sub(1))),
        next_domain: neighbour(position.map(|p| p + 1)),
    })
}

/// "Kurikulum Indonesia › SMA / MA — Kelas 10-12 › Kelas 10 SMA/MA" per
/// referencing learning path, deduplicated.
async fn learning_paths(pool: &PgPool, topic_ids: &[Uuid]) -> Result<std::collections::HashMap<Uuid, Vec<String>>, AppError> {
    let rows = sqlx::query!(
        r#"with recursive chain as (
             select r.source_module_id as topic_id, r.parent_id, array[]::text[] as path
             from modules r where r.source_module_id = any($1)
             union all
             select c.topic_id, p.parent_id, p.title || c.path from chain c join modules p on p.id = c.parent_id
           )
           select topic_id as "topic_id!", array_agg(distinct array_to_string(path[1:3], ' › ')) as "paths!"
           from chain where parent_id is null group by topic_id"#,
        topic_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.topic_id, r.paths)).collect())
}

/// Up to `limit` gold examples, closest first: same subject, then same
/// stage, then anything — never the domain being planned (that would be
/// copying the answer, and would make a benchmark meaningless).
pub async fn load_exemplars(pool: &PgPool, ctx: &DomainContext, limit: i64) -> Result<Vec<Exemplar>, AppError> {
    let stage = stage_key(&ctx.standard.jenjang);
    let rows = sqlx::query!(
        r#"select title, input, output from generation_exemplars
           where kind = 'bab_plan' and enabled and domain_folder_id is distinct from $1
           order by (subject_id is not distinct from $2) desc, (stage = $3) desc, (source = 'approved') desc, random()
           limit $4"#,
        ctx.domain_folder_id,
        ctx.subject_id,
        stage,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| Exemplar { title: r.title, input: r.input, output: r.output }).collect())
}

pub fn stage_key(jenjang: &str) -> String {
    use crate::services::quiz_taxonomy::Stage;
    match Stage::infer(jenjang) {
        Some(Stage::Sd) => "sd",
        Some(Stage::Smp) => "smp",
        Some(Stage::Sma) => "sma",
        Some(Stage::Kuliah) => "kuliah",
        Some(Stage::Pascasarjana) => "pascasarjana",
        Some(Stage::Umum) => "umum",
        None => "lainnya",
    }
    .to_string()
}

/// Who a `tujuan` speaks about, per jenjang.
pub fn learner_word(jenjang: &str) -> &'static str {
    match stage_key(jenjang).as_str() {
        "kuliah" | "pascasarjana" => "Mahasiswa",
        "umum" => "Peserta",
        _ => "Siswa",
    }
}

pub fn system_prompt() -> String {
    r#"Anda adalah perancang kurikulum senior di Indonesia yang menyusun struktur BAB untuk platform belajar mandiri. Setiap topik dipecah menjadi bab; setiap bab nanti diisi satu Modul Belajar (±45 menit) dan tiga latihan soal. Tugas Anda HANYA menyusun bab: judul, tujuan, dan cakupan. Mutu yang diharapkan setara penyusun kurikulum terbaik: tepat standar, berurutan, tidak tumpang tindih, dan berguna bagi semua learning path yang memakai topik itu.

ATURAN WAJIB
1. Jumlah bab per topik 4–6 (boleh 3 untuk topik yang sangat sempit, paling banyak 8 untuk topik yang sangat luas). Satu bab = satu sesi belajar ±45 menit: tidak sesempit satu definisi, tidak seluas satu topik.
2. Urutan bab mengikuti prasyarat: konsep dasar → hubungan/rumus → penerapan/soal → kasus nyata atau kesalahan umum bila relevan.
3. TIDAK tumpang tindih: antarbab dalam satu topik, dengan topik lain di domain yang sama, dan dengan domain sebelum/sesudah. Materi milik topik lain boleh disebut sebagai prasyarat, tetapi tidak dijadikan bab.
4. Judul bab spesifik dan informatif (≤ 90 karakter), bukan judul generik seperti "Pengantar", "Rangkuman", "Latihan", dan tidak sama dengan judul topik.
5. "tujuan": satu kalimat terukur dengan kata kerja operasional (menghitung, membedakan, menganalisis, merancang, …), bukan "memahami" atau "mengetahui". Subjek kalimat sesuai jenjang: "Siswa" (SD–SMA), "Mahasiswa" (S1–S3), "Peserta" (umum/dewasa).
6. "cakupan": 3–4 butir konkret yang WAJIB diajarkan di bab itu (istilah, rumus, jenis kasus), singkat dan tidak berulang antarbab.
7. Kedalaman, istilah, dan notasi mengikuti jenjang dan standar yang diberikan. Untuk topik yang juga dipakai jalur internasional atau ujian (Cambridge, UTBK/SNBT, TKA, OSN, CPNS), pastikan bab mencakup kebutuhan jalur itu tanpa melampaui jenjang.
8. "jenis_soal" per topik: "hitungan" bila latihannya menuntut perhitungan/penyelesaian bernilai tunggal; "konsep" bila jawabannya berupa konsep, penjelasan, klasifikasi, atau sejarah; "bahasa" untuk pelajaran bahasa. Kosongkan (null) bila sama dengan bawaan tahap.
9. Gunakan bahasa Indonesia baku (kecuali istilah teknis yang lazim ditulis dalam bahasa aslinya).

KELUARAN: HANYA JSON, tanpa teks lain, tanpa pagar kode, dengan bentuk:
{"topik":[{"no":1,"t":"judul topik persis seperti diberikan","jenis_soal":null,"bab":[{"judul":"…","tujuan":"…","cakupan":["…","…","…"]}]}]}"#
        .to_string()
}

fn format_exemplar(e: &Exemplar) -> String {
    // Stored gold carries topic ids for the benchmark; the model has no
    // use for them.
    let mut output = e.output.clone();
    if let Some(topics) = output.get_mut("topik").and_then(serde_json::Value::as_array_mut) {
        for t in topics.iter_mut().filter_map(serde_json::Value::as_object_mut) {
            t.remove("topic_id");
        }
    }
    format!("### CONTOH: {}\nMASUKAN:\n{}\nKELUARAN YANG BAIK:\n{}", e.title, compact(&e.input), compact(&output))
}

fn compact(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

pub fn context_block(ctx: &DomainContext) -> String {
    let mut out = String::new();
    out.push_str(&format!("MATA PELAJARAN: {}\nTAHAP: {}\nJENJANG: {}\n", ctx.subject_folder_title, ctx.tahap_title, ctx.standard.jenjang));
    if !ctx.standard.standar.is_empty() {
        out.push_str("ACUAN STANDAR:\n");
        for s in &ctx.standard.standar {
            out.push_str(&format!("- {s}\n"));
        }
    }
    out.push_str(&format!("JENIS SOAL BAWAAN TAHAP: {}\n", ctx.standard.jenis_soal));
    if let Some(prev) = &ctx.previous_domain {
        out.push_str(&format!("DOMAIN SEBELUMNYA \"{}\" (jangan diulang): {}\n", prev.title, prev.topics.join("; ")));
    }
    if let Some(next) = &ctx.next_domain {
        out.push_str(&format!("DOMAIN SESUDAHNYA \"{}\" (jangan didahului): {}\n", next.title, next.topics.join("; ")));
    }
    out.push_str(&format!("\nDOMAIN: {}\nSemua topik di domain ini (untuk batas materi):\n", ctx.domain_title));
    for t in &ctx.topics {
        let mark = if ctx.target_topic_ids.contains(&t.topic_id) { "" } else { " (sudah punya bab — hanya untuk batas)" };
        let babs = if t.existing_babs.is_empty() { String::new() } else { format!(" — bab: {}", t.existing_babs.join("; ")) };
        out.push_str(&format!("- {}{}{}\n", t.title, mark, babs));
    }
    out
}

/// The user prompt for a full plan, or — when `repair` carries issues per
/// topic — for only those topics, with what was wrong spelled out.
pub fn user_prompt(ctx: &DomainContext, exemplars: &[Exemplar], repair: &[(usize, &TopicContext, Option<&TopicDraft>, Vec<String>)]) -> String {
    let mut out = String::new();
    if !exemplars.is_empty() {
        out.push_str("Pelajari mutu, kedalaman, dan gaya contoh berikut (dari mata pelajaran/domain lain). Tiru mutunya, BUKAN isinya.\n\n");
        for e in exemplars {
            out.push_str(&format_exemplar(e));
            out.push_str("\n\n");
        }
    }
    out.push_str("=== TUGAS ===\n");
    out.push_str(&context_block(ctx));
    if repair.is_empty() {
        out.push_str("\nSusun bab untuk topik bernomor berikut (gunakan nomor yang sama di keluaran):\n");
        for (i, t) in ctx.targets().iter().enumerate() {
            out.push_str(&format!("{}. {}\n   dipakai di: {}\n", i + 1, t.title, if t.paths.is_empty() { "-".to_string() } else { t.paths.join(" | ") }));
        }
    } else {
        out.push_str("\nPERBAIKI rencana bab untuk topik berikut. Tulis ulang bab topik tersebut secara utuh dengan memperbaiki SETIAP masalah yang disebut. Keluarkan hanya topik-topik ini, dengan nomornya.\n");
        for (no, t, previous, issues) in repair {
            out.push_str(&format!("{}. {}\n   dipakai di: {}\n", no, t.title, if t.paths.is_empty() { "-".to_string() } else { t.paths.join(" | ") }));
            if let Some(prev) = previous {
                out.push_str(&format!("   rencana sebelumnya: {}\n", serde_json::to_string(&prev.bab).unwrap_or_default()));
            }
            for issue in issues {
                out.push_str(&format!("   - masalah: {issue}\n"));
            }
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    topik: Vec<RawTopic>,
}

#[derive(Debug, Deserialize)]
struct RawTopic {
    no: usize,
    #[serde(default)]
    t: String,
    #[serde(default)]
    jenis_soal: Option<String>,
    #[serde(default)]
    bab: Vec<BabDraft>,
}

/// The model's JSON, mapped back onto topic ids by the numbers it was
/// given. `numbering` is the list the prompt numbered (1-based).
pub fn parse(text: &str, numbering: &[&TopicContext]) -> Result<Vec<TopicDraft>, String> {
    let cleaned = crate::services::ai_provider::strip_code_fence(text);
    let start = cleaned.find('{').ok_or("keluaran tidak berisi JSON")?;
    let end = cleaned.rfind('}').ok_or("keluaran tidak berisi JSON")?;
    let raw: RawPlan = serde_json::from_str(&cleaned[start..=end]).map_err(|e| format!("JSON tidak valid: {e}"))?;
    let mut out = Vec::new();
    for t in raw.topik {
        let Some(topic) = t.no.checked_sub(1).and_then(|i| numbering.get(i)) else { continue };
        out.push(TopicDraft {
            topic_id: topic.topic_id,
            t: if t.t.trim().is_empty() { topic.title.clone() } else { t.t.trim().to_string() },
            jenis_soal: t.jenis_soal.map(|j| j.trim().to_lowercase()).filter(|j| !j.is_empty() && j != "null"),
            bab: t.bab.into_iter().map(|b| BabDraft { judul: b.judul.trim().to_string(), tujuan: b.tujuan.trim().to_string(), cakupan: b.cakupan.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect() }).collect(),
            status: TopicStatus::Pending,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(title: &str) -> TopicContext {
        TopicContext { topic_id: Uuid::new_v4(), title: title.into(), paths: vec![], existing_babs: vec![] }
    }

    #[test]
    fn parse_maps_numbers_back_to_topics_and_tolerates_fences() {
        let a = topic("Hukum I Newton");
        let b = topic("Hukum II Newton");
        let text = "```json\n{\"topik\":[{\"no\":2,\"t\":\"Hukum II Newton\",\"jenis_soal\":\"null\",\"bab\":[{\"judul\":\" F = ma \",\"tujuan\":\"Siswa dapat menghitung.\",\"cakupan\":[\"a\",\" \"]}]},{\"no\":9,\"t\":\"x\",\"bab\":[]}]}\n```";
        let drafts = parse(text, &[&a, &b]).unwrap();
        assert_eq!(drafts.len(), 1, "an out-of-range number is dropped, not guessed");
        assert_eq!(drafts[0].topic_id, b.topic_id);
        assert_eq!(drafts[0].jenis_soal, None);
        assert_eq!(drafts[0].bab[0].judul, "F = ma");
        assert_eq!(drafts[0].bab[0].cakupan, vec!["a".to_string()]);
    }

    #[test]
    fn learner_word_follows_the_jenjang() {
        assert_eq!(learner_word("SMA/MA kelas 10 (Fase E)"), "Siswa");
        assert_eq!(learner_word("perguruan tinggi S1 Fisika"), "Mahasiswa");
        assert_eq!(learner_word("umum/dewasa (CPNS)"), "Peserta");
    }
}
