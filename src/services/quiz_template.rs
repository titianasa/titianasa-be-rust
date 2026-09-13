// Phase 37 — exam blueprints.
//
// A template is STRUCTURE ONLY: which sections a paper has, how long
// each runs, which question types sit in each section and how many of
// each. It carries no question content — applying one gives the author a
// correctly-shaped empty paper, which they then fill in by hand or with
// "Generate soal" per group (each group's `context_prompt` is what steers
// that generation).
//
// Why these are code and not rows in a table: they are the same kind of
// thing `quiz_subtype.rs` is — a fixed catalogue that changes on deploy,
// is referenced by id, and must stay in step with the subtype registry.
// A template naming a subtype that does not exist is a compile-time
// concern, and `every_template_is_valid` below turns it into one.
//
// The set is driven by the exams this app's learning paths actually
// carry (`module_labels.kind = 'ujian'`), which is dominated by
// Indonesian selection exams — UTBK/SNBT, Mandiri PTN, UM-PTKIN, CPNS,
// BUMN, OSN, AKM — not by the English test-prep catalogue.

use crate::services::quiz_subtype;

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct TemplateSectionSpec {
    pub id: &'static str,
    pub title: &'static str,
    /// Author guidance carried into the section, and into the generator
    /// prompt for every group inside it.
    pub context_prompt: &'static str,
    /// Time budget when the deck uses `timer_mode = "section"`.
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct TemplateGroupSpec {
    /// Which section this group belongs to.
    pub section_ref: &'static str,
    /// A `quiz_subtype` id.
    pub subtype: &'static str,
    /// How many questions the group starts with.
    pub count: i64,
    /// Presentation variant, when the paper uses one.
    pub display_mode: Option<&'static str>,
    pub instruction: &'static str,
    /// Steers "Generate soal" for this group specifically.
    pub context_prompt: &'static str,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct QuizTemplate {
    pub id: &'static str,
    /// Exam family — the folder the picker groups by.
    pub folder: &'static str,
    /// Optional sub-grouping inside the family (a skill, a subtest).
    pub subfolder: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub language: &'static str,
    /// "default" | "ielts" | "pte" — chrome only, never scoring.
    pub theme: &'static str,
    pub timer_mode: &'static str,
    pub max_duration_seconds: Option<f64>,
    pub passing_score: Option<f64>,
    pub sections: &'static [TemplateSectionSpec],
    pub groups: &'static [TemplateGroupSpec],
}

impl QuizTemplate {
    /// Total questions the applied paper will start with.
    pub fn question_count(&self) -> i64 {
        self.groups.iter().map(|g| g.count).sum()
    }

    /// "1 jam 30 menit" / "45 menit" — for the template browser's card
    /// and detail panel. Derived from `max_duration_seconds` rather
    /// than an authored field, so it can never drift out of sync with
    /// the timer the applied paper actually runs.
    pub fn duration_label(&self) -> Option<String> {
        let total_minutes = (self.max_duration_seconds? / 60.0).round() as i64;
        let hours = total_minutes / 60;
        let minutes = total_minutes % 60;
        Some(match (hours, minutes) {
            (0, m) => format!("{m} menit"),
            (h, 0) => format!("{h} jam"),
            (h, m) => format!("{h} jam {m} menit"),
        })
    }

    /// A rough, zero-curation stand-in for authored difficulty (Phase
    /// 38) — a higher passing bar reads as a more selective exam.
    /// Deliberately a computed method rather than a new stored field:
    /// with 100+ existing template literals above, a required field
    /// would mean touching every one of them for a number this
    /// approximates about as well anyway.
    pub fn difficulty(&self) -> quiz_subtype::Difficulty {
        use quiz_subtype::Difficulty;
        match self.passing_score {
            Some(p) if p >= 70.0 => Difficulty::Hard,
            Some(p) if p >= 50.0 => Difficulty::Medium,
            Some(_) => Difficulty::Easy,
            None => Difficulty::Medium,
        }
    }

    /// Which subtype FAMILIES this template's groups actually touch —
    /// read straight off the subtype registry, so (unlike a hand-tagged
    /// list) it can never claim a family the template doesn't contain.
    pub fn tags(&self) -> Vec<quiz_subtype::SubtypeFamily> {
        let mut out = Vec::new();
        for g in self.groups {
            if let Some(info) = quiz_subtype::find(g.subtype) {
                if !out.contains(&info.family) {
                    out.push(info.family);
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct TemplateFolder {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

pub const FOLDERS: &[TemplateFolder] = &[
    TemplateFolder { id: "snbt", name: "UTBK-SNBT", description: "Seleksi Nasional Berdasarkan Tes — 7 subtes." },
    TemplateFolder { id: "akm", name: "AKM / ANBK", description: "Asesmen Kompetensi Minimum — literasi & numerasi berbasis stimulus." },
    TemplateFolder { id: "cpns", name: "CPNS SKD", description: "Seleksi Kompetensi Dasar — TWK, TIU, TKP." },
    TemplateFolder { id: "bumn", name: "Rekrutmen BUMN", description: "TKD, Core Values AKHLAK, dan Learning Agility." },
    TemplateFolder { id: "mandiri", name: "Mandiri PTN", description: "Seleksi mandiri perguruan tinggi negeri — TPA dan mata uji." },
    TemplateFolder { id: "umptkin", name: "UM-PTKIN", description: "Ujian Masuk PTKIN — penalaran, literasi tiga bahasa, dan keislaman." },
    TemplateFolder { id: "psikotes", name: "Psikotes", description: "Tes kognitif dan kepribadian untuk seleksi kerja." },
    TemplateFolder { id: "tka", name: "TKA", description: "Tes Kemampuan Akademik — mata uji dan literasi." },
    TemplateFolder { id: "osn", name: "OSN", description: "Olimpiade Sains Nasional." },
    TemplateFolder { id: "ielts", name: "IELTS", description: "Academic — Listening, Reading, Writing, Speaking." },
    TemplateFolder { id: "toefl", name: "TOEFL", description: "iBT dan ITP." },
    TemplateFolder { id: "toeic", name: "TOEIC", description: "Listening & Reading." },
    TemplateFolder { id: "pte", name: "PTE Academic", description: "Pearson Test of English." },
    TemplateFolder { id: "duolingo", name: "Duolingo English Test", description: "Tes adaptif berbasis komputer." },
    TemplateFolder { id: "hsk", name: "HSK", description: "汉语水平考试 — HSK 1 sampai 6." },
    TemplateFolder { id: "jlpt", name: "JLPT", description: "日本語能力試験 — N5 sampai N1." },
    TemplateFolder { id: "topik", name: "TOPIK", description: "한국어능력시험 — TOPIK I & II." },
    TemplateFolder { id: "delf", name: "DELF", description: "Diplôme d'études en langue française." },
    TemplateFolder { id: "goethe", name: "Goethe-Zertifikat", description: "Ujian bahasa Jerman." },
];

const fn sec(id: &'static str, title: &'static str, context_prompt: &'static str, minutes: f64) -> TemplateSectionSpec {
    TemplateSectionSpec { id, title, context_prompt, duration_seconds: Some(minutes * 60.0) }
}

const fn grp(
    section_ref: &'static str,
    subtype: &'static str,
    count: i64,
    instruction: &'static str,
    context_prompt: &'static str,
) -> TemplateGroupSpec {
    TemplateGroupSpec { section_ref, subtype, count, display_mode: None, instruction, context_prompt }
}

/// A group rendered in one of its subtype's presentation variants —
/// a drag-and-drop word bank, or a statement grid.
const fn grp_as(
    section_ref: &'static str,
    subtype: &'static str,
    count: i64,
    display_mode: &'static str,
    instruction: &'static str,
    context_prompt: &'static str,
) -> TemplateGroupSpec {
    TemplateGroupSpec { section_ref, subtype, count, display_mode: Some(display_mode), instruction, context_prompt }
}

// ── UTBK-SNBT ───────────────────────────────────────────────────────────
// The 2023-onward format: 7 subtests, each separately timed.

const SNBT_PU_SECTIONS: &[TemplateSectionSpec] = &[sec("pu", "Penalaran Umum", "Penalaran induktif, deduktif, dan kuantitatif. Stimulus berupa paragraf argumentatif, tabel, atau grafik.", 30.0)];
const SNBT_PU_GROUPS: &[TemplateGroupSpec] = &[
    grp("pu", "multiple_choice", 20, "Pilih jawaban yang paling tepat.", "Penalaran induktif & deduktif: simpulan, asumsi, pelemahan/penguatan argumen. Satu stimulus dipakai 2-3 soal."),
    grp_as("pu", "true_false", 6, "grid", "Tentukan Benar atau Salah untuk tiap pernyataan.", "Pernyataan Benar/Salah atas satu stimulus data. Sertakan pengecoh yang menuntut pembacaan cermat."),
    grp("pu", "short_answer", 4, "Isikan jawaban singkat.", "Penalaran kuantitatif: pola bilangan, perbandingan, rata-rata. Jawaban berupa angka."),
];

const SNBT_PBM_SECTIONS: &[TemplateSectionSpec] = &[sec("pbm", "Pemahaman Bacaan dan Menulis", "Teks 300-500 kata bertema sosial, budaya, atau sains populer Indonesia.", 25.0)];
const SNBT_PBM_GROUPS: &[TemplateGroupSpec] = &[
    grp("pbm", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Ide pokok, simpulan, makna kata dalam konteks, kalimat efektif, dan ejaan sesuai PUEBI."),
    grp("pbm", "gap_fill", 5, "Lengkapi bagian rumpang.", "Kata penghubung/transisi dan diksi baku yang tepat untuk melengkapi paragraf."),
    grp("pbm", "matching", 3, "Cocokkan.", "Cocokkan kalimat dengan jenis kesalahan berbahasanya."),
];

const SNBT_PK_SECTIONS: &[TemplateSectionSpec] = &[sec("pk", "Pengetahuan Kuantitatif", "Aritmetika, aljabar, geometri, dan analisis data setingkat SMA.", 20.0)];
const SNBT_PK_GROUPS: &[TemplateGroupSpec] = &[
    grp("pk", "multiple_choice", 14, "Pilih jawaban yang paling tepat.", "Sertakan soal perbandingan kuantitas dan kecukupan data. Tulis rumus dengan LaTeX."),
    grp("pk", "short_answer", 6, "Isikan jawaban berupa angka.", "Hasil akhir berupa bilangan; terima desimal koma maupun titik."),
];

const SNBT_LITID_SECTIONS: &[TemplateSectionSpec] = &[sec("lit", "Literasi Bahasa Indonesia", "Teks sastra dan informasional, 400-700 kata.", 42.0)];
const SNBT_LITID_GROUPS: &[TemplateGroupSpec] = &[
    grp("lit", "multiple_choice", 20, "Pilih jawaban yang paling tepat.", "Menemukan informasi, menafsirkan, mengevaluasi, dan merefleksi isi teks."),
    grp_as("lit", "true_false", 6, "grid", "Tentukan Benar atau Salah.", "Pernyataan evaluatif atas isi teks."),
    grp("lit", "multiple_choice_multiple", 4, "Pilih semua yang benar.", "Soal pilihan ganda kompleks: lebih dari satu jawaban benar."),
];

const SNBT_LITEN_SECTIONS: &[TemplateSectionSpec] = &[sec("liten", "Literasi Bahasa Inggris", "Teks informasional berbahasa Inggris, 250-400 kata, level B1-B2.", 25.0)];
const SNBT_LITEN_GROUPS: &[TemplateGroupSpec] = &[
    grp("liten", "multiple_choice", 14, "Choose the best answer.", "Main idea, inference, vocabulary in context, and author's purpose."),
    grp("liten", "true_false_not_given", 6, "Do the statements agree with the passage?", "True / False / Not Given atas teks yang sama."),
];

const SNBT_PM_SECTIONS: &[TemplateSectionSpec] = &[sec("pm", "Penalaran Matematika", "Masalah kontekstual yang menuntut pemodelan matematis.", 42.0)];
const SNBT_PM_GROUPS: &[TemplateGroupSpec] = &[
    grp("pm", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Konteks nyata (ekonomi, kesehatan, lingkungan) yang dimodelkan secara matematis. Gunakan LaTeX untuk rumus."),
    grp("pm", "short_answer", 5, "Isikan jawaban berupa angka.", "Jawaban numerik; sertakan satuan pada pertanyaan, bukan pada kunci."),
    grp_as("pm", "true_false", 3, "grid", "Tentukan Benar atau Salah.", "Pernyataan atas hasil perhitungan dari stimulus yang sama."),
];

// ── AKM ─────────────────────────────────────────────────────────────────

const AKM_LIT_SECTIONS: &[TemplateSectionSpec] = &[sec("stimulus", "Stimulus", "Teks fiksi atau informasional sesuai jenjang, boleh disertai gambar/infografik.", 30.0)];
const AKM_LIT_GROUPS: &[TemplateGroupSpec] = &[
    grp("stimulus", "multiple_choice", 6, "Pilih jawaban yang tepat.", "Menemukan informasi tersurat, menafsirkan, dan mengevaluasi teks."),
    grp("stimulus", "multiple_choice_multiple", 3, "Pilih semua yang benar.", "Pilihan ganda kompleks."),
    grp_as("stimulus", "true_false", 4, "grid", "Tentukan Benar atau Salah.", "Format Benar-Salah khas AKM atas satu stimulus."),
    grp("stimulus", "matching", 3, "Jodohkan.", "Menjodohkan bagian teks dengan fungsinya."),
    grp("stimulus", "short_answer", 2, "Jawab singkat.", "Isian singkat berdasarkan isi teks."),
    grp("stimulus", "essay", 1, "Tulis jawabanmu.", "Uraian reflektif; dinilai dengan rubrik."),
];

const AKM_NUM_SECTIONS: &[TemplateSectionSpec] = &[sec("stimulus", "Stimulus", "Konteks personal, sosial-budaya, atau saintifik dengan data/tabel/grafik.", 30.0)];
const AKM_NUM_GROUPS: &[TemplateGroupSpec] = &[
    grp("stimulus", "multiple_choice", 6, "Pilih jawaban yang tepat.", "Bilangan, geometri-pengukuran, aljabar, data-ketidakpastian. Gunakan LaTeX."),
    grp_as("stimulus", "true_false", 4, "grid", "Tentukan Benar atau Salah.", "Pernyataan atas hasil pembacaan data."),
    grp("stimulus", "short_answer", 4, "Isikan angka.", "Jawaban numerik dari perhitungan pada stimulus."),
    grp("stimulus", "table_completion", 2, "Lengkapi tabel.", "Melengkapi sel tabel dari data pada stimulus."),
    grp("stimulus", "essay", 1, "Jelaskan caramu.", "Uraian penalaran; dinilai rubrik."),
];

// ── CPNS SKD ────────────────────────────────────────────────────────────

const CPNS_SECTIONS: &[TemplateSectionSpec] = &[
    sec("twk", "TWK — Wawasan Kebangsaan", "Pancasila, UUD 1945, NKRI, Bhinneka Tunggal Ika, dan bela negara.", 30.0),
    sec("tiu", "TIU — Intelegensia Umum", "Verbal (analogi, silogisme), numerik (deret, hitung), dan figural.", 35.0),
    sec("tkp", "TKP — Karakteristik Pribadi", "Pelayanan publik, jejaring kerja, sosial budaya, TIK, profesionalisme, dan anti-radikalisme.", 35.0),
];
const CPNS_GROUPS: &[TemplateGroupSpec] = &[
    grp("twk", "multiple_choice", 30, "Pilih jawaban yang benar.", "Satu jawaban benar, skor 5 untuk benar dan 0 untuk salah."),
    grp("tiu", "analogy", 5, "Pilih pasangan kata yang hubungannya paling serupa.", "Analogi verbal setara TIU."),
    grp("tiu", "multiple_choice", 25, "Pilih jawaban yang benar.", "Silogisme, deret angka, aritmetika dasar, dan penalaran figural."),
    grp("tiu", "short_answer", 5, "Isikan angka.", "Deret dan hitung cepat dengan jawaban numerik."),
    // The subtest that has no wrong answer — every option scores 1-5.
    grp("tkp", "multiple_choice", 45, "Pilih tindakan yang paling menggambarkan diri Anda.", "WAJIB: setiap opsi diberi bobot 1-5 pada `option_scores`; tidak ada opsi yang salah. Situasi kerja nyata di instansi pemerintah."),
];

// ── BUMN ────────────────────────────────────────────────────────────────

const BUMN_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tkd", "TKD", "Verbal, numerik, dan logika dasar.", 40.0),
    sec("akhlak", "Core Values AKHLAK", "Amanah, Kompeten, Harmonis, Loyal, Adaptif, Kolaboratif.", 25.0),
];
const BUMN_GROUPS: &[TemplateGroupSpec] = &[
    grp("tkd", "multiple_choice", 30, "Pilih jawaban yang benar.", "Sinonim/antonim, deret, aritmetika, dan penalaran logis."),
    grp("akhlak", "multiple_choice", 25, "Pilih yang paling menggambarkan Anda.", "WAJIB: `option_scores` 1-5 per opsi, dipetakan ke salah satu nilai AKHLAK. Tidak ada opsi salah."),
];

// ── OSN ─────────────────────────────────────────────────────────────────

const OSN_SECTIONS: &[TemplateSectionSpec] = &[
    sec("isian", "Isian Singkat", "Soal dengan jawaban pasti berupa angka atau ekspresi.", 90.0),
    sec("uraian", "Uraian", "Soal pembuktian atau pemecahan masalah bertahap.", 120.0),
];
const OSN_GROUPS: &[TemplateGroupSpec] = &[
    grp("isian", "short_answer", 10, "Tuliskan jawaban akhir.", "Setingkat olimpiade nasional. Jawaban tunggal dan pasti. Gunakan LaTeX."),
    grp("uraian", "essay", 4, "Tuliskan penyelesaian lengkap.", "Menuntut pembuktian/langkah lengkap; dinilai rubrik per langkah."),
];

const OSN_INF_SECTIONS: &[TemplateSectionSpec] = &[
    sec("analitika", "Analitika", "Penalaran logis dan algoritmik tanpa koding.", 60.0),
    sec("pemrograman", "Pemrograman", "Implementasi algoritma.", 120.0),
];
const OSN_INF_GROUPS: &[TemplateGroupSpec] = &[
    grp("analitika", "multiple_choice", 15, "Pilih jawaban yang benar.", "Logika, kombinatorika, dan penelusuran algoritma."),
    grp("analitika", "short_answer", 5, "Isikan jawaban.", "Hasil penelusuran algoritma berupa angka atau string."),
    grp("pemrograman", "file_upload", 3, "Unggah berkas solusi.", "Kirim source code; dinilai manual oleh juri."),
];

// ── IELTS ───────────────────────────────────────────────────────────────

const IELTS_READING_SECTIONS: &[TemplateSectionSpec] = &[
    sec("p1", "Reading Passage 1", "Factual/descriptive article, ~700-900 kata.", 20.0),
    sec("p2", "Reading Passage 2", "Work/education-oriented text, ~800-900 kata.", 20.0),
    sec("p3", "Reading Passage 3", "Academic argument, ~900-1000 kata.", 20.0),
];
const IELTS_READING_GROUPS: &[TemplateGroupSpec] = &[
    grp("p1", "true_false_not_given", 6, "Do the following statements agree with the information?", "TRUE / FALSE / NOT GIVEN. NOT GIVEN harus benar-benar tidak disinggung passage."),
    grp_as("p1", "matching_headings", 4, "ielts", "Choose the correct heading for each section.", "Sediakan 2-3 heading pengecoh. Passage dipecah per bagian berlabel."),
    grp("p1", "short_answer", 3, "Answer using NO MORE THAN THREE WORDS.", "Jawaban muncul verbatim di passage."),
    grp_as("p2", "gap_fill", 6, "ielts", "Complete the notes below.", "Note completion dengan word bank; blank inline di dalam prosa."),
    grp_as("p2", "matching", 4, "ielts", "Match each statement with the correct person.", "Word bank berisi nama/kategori; boleh dipakai lebih dari sekali."),
    grp("p2", "table_completion", 3, "Complete the table.", "Tabel ringkasan data dari passage."),
    grp("p3", "multiple_choice", 6, "Choose the correct letter.", "Inferensi dan maksud penulis."),
    grp("p3", "yes_no_not_given", 4, "Do the statements agree with the writer's claims?", "YES / NO / NOT GIVEN — tentang pandangan penulis, bukan fakta."),
    grp("p3", "multiple_choice_multiple", 3, "Choose TWO letters.", "Stem menyebut eksplisit jumlah jawaban yang dipilih."),
];

const IELTS_LISTENING_SECTIONS: &[TemplateSectionSpec] = &[
    sec("s1", "Section 1 — Percakapan sehari-hari", "Dialog dua penutur tentang kebutuhan sosial (pemesanan, pendaftaran).", 8.0),
    sec("s2", "Section 2 — Monolog", "Monolog konteks sosial (panduan fasilitas, pengumuman).", 8.0),
    sec("s3", "Section 3 — Diskusi akademik", "Diskusi 2-4 penutur dalam konteks pendidikan.", 8.0),
    sec("s4", "Section 4 — Kuliah", "Monolog akademik.", 8.0),
];
const IELTS_LISTENING_GROUPS: &[TemplateGroupSpec] = &[
    grp("s1", "gap_fill", 10, "Complete the form below.", "Form completion: nama, tanggal, nomor, alamat. Jawaban maksimal 3 kata/angka."),
    grp_as("s2", "map_labeling", 5, "ielts", "Label the map below.", "Denah/peta dengan pool label."),
    grp("s2", "multiple_choice", 5, "Choose the correct letter.", "Detail dari monolog."),
    grp_as("s3", "matching", 5, "ielts", "Match each opinion to the correct speaker.", "Mencocokkan pendapat dengan penutur."),
    grp("s3", "multiple_choice", 5, "Choose the correct letter.", "Pemahaman diskusi."),
    grp("s4", "gap_fill", 10, "Complete the summary.", "Ringkasan kuliah dengan rumpang."),
];

const IELTS_WRITING_SECTIONS: &[TemplateSectionSpec] = &[
    sec("t1", "Task 1", "Deskripsi grafik/tabel/diagram, minimal 150 kata.", 20.0),
    sec("t2", "Task 2", "Esai argumentatif, minimal 250 kata.", 40.0),
];
const IELTS_WRITING_GROUPS: &[TemplateGroupSpec] = &[
    grp("t1", "essay", 1, "Summarise the information.", "Minimal 150 kata. Rubrik: Task Achievement, Coherence, Lexical Resource, Grammatical Range."),
    grp("t2", "essay", 1, "Write an essay.", "Minimal 250 kata. Rubrik IELTS Writing Task 2 penuh."),
];

// General Training — same skill, different SOURCE TEXTS: everyday/
// workplace material instead of Academic's journal-style passages, and
// Task 1 is a letter rather than a graph description.
const IELTS_GT_READING_SECTIONS: &[TemplateSectionSpec] = &[
    sec("s1", "Section 1 — Social survival", "2-3 teks pendek kehidupan sehari-hari (iklan, jadwal, notifikasi).", 20.0),
    sec("s2", "Section 2 — Workplace survival", "2 teks terkait pekerjaan (kontrak, kebijakan perusahaan, lowongan).", 20.0),
    sec("s3", "Section 3 — General reading", "1 teks panjang bertema umum, gaya mirip Academic Passage 3.", 20.0),
];
const IELTS_GT_READING_GROUPS: &[TemplateGroupSpec] = &[
    grp("s1", "matching", 5, "Match each notice to the correct category.", "Mencocokkan iklan/notifikasi pendek dengan kategorinya."),
    grp("s1", "true_false_not_given", 5, "Do the statements agree with the notices?", "TRUE / FALSE / NOT GIVEN atas detail teks sehari-hari."),
    grp("s2", "short_answer", 5, "Answer using NO MORE THAN THREE WORDS.", "Detail dari dokumen kerja — jam kerja, tunjangan, syarat."),
    grp_as("s2", "gap_fill", 5, "ielts", "Complete the notes below.", "Ringkasan kebijakan perusahaan dengan rumpang."),
    grp("s3", "multiple_choice", 5, "Choose the correct letter.", "Inferensi dari teks umum panjang."),
    grp_as("s3", "matching_headings", 4, "ielts", "Choose the correct heading for each paragraph.", "Sediakan 2-3 heading pengecoh."),
];

const IELTS_GT_WRITING_SECTIONS: &[TemplateSectionSpec] = &[
    sec("t1", "Task 1 — Letter", "Surat formal, semi-formal, atau informal, minimal 150 kata.", 20.0),
    sec("t2", "Task 2", "Esai argumentatif, minimal 250 kata — sama seperti Academic.", 40.0),
];
const IELTS_GT_WRITING_GROUPS: &[TemplateGroupSpec] = &[
    grp("t1", "essay", 1, "Write a letter.", "Minimal 150 kata. Tentukan nada (formal/semi-formal/informal) sesuai skenario; cakup semua poin instruksi."),
    grp("t2", "essay", 1, "Write an essay.", "Minimal 250 kata. Rubrik sama seperti IELTS Academic Writing Task 2."),
];

const IELTS_SPEAKING_SECTIONS: &[TemplateSectionSpec] = &[
    sec("p1", "Part 1 — Interview", "Pertanyaan tentang diri, rumah, pekerjaan, minat.", 5.0),
    sec("p2", "Part 2 — Cue Card", "Berbicara 1-2 menit setelah 1 menit persiapan.", 4.0),
    sec("p3", "Part 3 — Discussion", "Diskusi abstrak lanjutan dari topik Part 2.", 5.0),
];
const IELTS_SPEAKING_GROUPS: &[TemplateGroupSpec] = &[
    grp("p1", "voice_record", 4, "Answer the question.", "Jawaban pendek 20-30 detik per pertanyaan."),
    grp("p2", "voice_record", 1, "Talk about the topic for 1-2 minutes.", "Cue card dengan 3-4 poin panduan; sediakan waktu persiapan 60 detik."),
    grp("p3", "voice_record", 4, "Discuss the question.", "Pertanyaan abstrak yang menuntut opini dan alasan."),
];

// ── TOEFL ───────────────────────────────────────────────────────────────

const TOEFL_IBT_SECTIONS: &[TemplateSectionSpec] = &[
    sec("reading", "Reading", "2 passage akademik, masing-masing ~700 kata.", 36.0),
    sec("listening", "Listening", "Kuliah dan percakapan kampus.", 36.0),
    sec("speaking", "Speaking", "1 independent + 3 integrated task.", 16.0),
    sec("writing", "Writing", "Integrated + Academic Discussion.", 29.0),
];
const TOEFL_IBT_GROUPS: &[TemplateGroupSpec] = &[
    grp("reading", "multiple_choice", 16, "Choose the best answer.", "Detail, inferensi, vocabulary-in-context, dan sisipan kalimat."),
    grp("reading", "multiple_choice_multiple", 4, "Choose TWO answers.", "Prose summary: pilih poin utama."),
    grp("listening", "multiple_choice", 22, "Choose the best answer.", "Gist, detail, fungsi ujaran, dan sikap penutur."),
    grp("listening", "table_completion", 6, "Complete the table.", "Connecting content: mengelompokkan informasi dari kuliah."),
    grp("speaking", "voice_record", 4, "Speak after the beep.", "Task 1 independent (45 detik); Task 2-4 integrated (60 detik) dengan bacaan/audio."),
    grp("writing", "essay", 2, "Write your response.", "Integrated (150-225 kata) dan Academic Discussion (min. 100 kata)."),
];

const TOEFL_ITP_SECTIONS: &[TemplateSectionSpec] = &[
    sec("listening", "Section 1 — Listening Comprehension", "Percakapan pendek, percakapan panjang, dan ceramah.", 35.0),
    sec("structure", "Section 2 — Structure & Written Expression", "Struktur kalimat dan identifikasi kesalahan.", 25.0),
    sec("reading", "Section 3 — Reading Comprehension", "5 passage akademik.", 55.0),
];
const TOEFL_ITP_GROUPS: &[TemplateGroupSpec] = &[
    grp("listening", "multiple_choice", 50, "Choose the best answer.", "Part A percakapan pendek, Part B percakapan panjang, Part C ceramah."),
    grp("structure", "multiple_choice", 15, "Choose the word or phrase that best completes the sentence.", "Structure: melengkapi kalimat. Fokus tata bahasa, bukan bacaan."),
    grp("structure", "error_identification", 25, "Identify the underlined part that must be changed.", "Written Expression: 4 bagian bertanda (A)(B)(C)(D), satu salah."),
    grp("reading", "multiple_choice", 50, "Choose the best answer.", "Main idea, detail, inferensi, rujukan kata, dan kosakata."),
];

// ── TOEIC ───────────────────────────────────────────────────────────────

const TOEIC_SECTIONS: &[TemplateSectionSpec] = &[
    sec("listening", "Listening", "Foto, tanya-jawab, percakapan, dan talk.", 45.0),
    sec("reading", "Reading", "Kalimat rumpang, teks rumpang, dan pemahaman bacaan.", 75.0),
];
const TOEIC_GROUPS: &[TemplateGroupSpec] = &[
    grp("listening", "image_word", 6, "Select the statement that best describes the picture.", "Part 1 Photographs: satu foto, empat deskripsi lisan."),
    grp("listening", "multiple_choice", 25, "Choose the best response.", "Part 2 Question-Response dan Part 3 Conversations."),
    grp("listening", "multiple_choice", 19, "Choose the best answer.", "Part 4 Short Talks: pengumuman, iklan, pesan telepon."),
    grp("reading", "multiple_choice", 30, "Choose the best answer.", "Part 5 Incomplete Sentences: tata bahasa dan kosakata bisnis."),
    grp("reading", "cloze_passage", 16, "Complete the text.", "Part 6 Text Completion: email/memo dengan empat rumpang."),
    grp("reading", "multiple_choice", 30, "Choose the best answer.", "Part 7 Reading Comprehension: teks tunggal dan ganda."),
];

// TOEIC Speaking & Writing — a separate test TRACK from Listening &
// Reading above, not a subset of it; scored and certificated on its own.
const TOEIC_SW_SECTIONS: &[TemplateSectionSpec] = &[
    sec("speaking", "Speaking", "Baca nyaring, deskripsi gambar, tanya-jawab, dan opini.", 20.0),
    sec("writing", "Writing", "Susun kalimat dari gambar, balas email, esai opini.", 60.0),
];
const TOEIC_SW_GROUPS: &[TemplateGroupSpec] = &[
    grp("speaking", "voice_record", 2, "Read the text aloud.", "Questions 1-2 Read a Text Aloud: teks pengumuman/iklan pendek, 45 detik persiapan."),
    grp("speaking", "voice_record", 1, "Describe the picture.", "Question 3 Describe a Picture: 30 detik persiapan, 45 detik jawaban."),
    grp("speaking", "voice_record", 3, "Respond to the question.", "Questions 4-6 Respond to Questions: pertanyaan seputar topik sehari-hari."),
    grp("speaking", "voice_record", 1, "Give your opinion.", "Question 11 Express an Opinion: 15 detik persiapan, 60 detik jawaban, disertai alasan."),
    grp("writing", "image_word", 5, "Write a sentence using the given words and picture.", "Questions 1-5 Write a Sentence Based on a Picture: gambar + 2 kata kunci wajib dipakai."),
    grp("writing", "essay", 2, "Respond to the e-mail.", "Questions 6-7 Respond to a Written Request: balasan email bisnis, minimal 3 kalimat."),
    grp("writing", "essay", 1, "Write an essay.", "Question 8 Opinion Essay: minimal 300 kata, argumen + contoh konkret."),
];

// ── PTE Academic ────────────────────────────────────────────────────────

const PTE_SECTIONS: &[TemplateSectionSpec] = &[
    sec("sw", "Speaking & Writing", "Read aloud, repeat sentence, describe image, essay.", 54.0),
    sec("reading", "Reading", "Fill in the blanks dan reorder paragraph.", 30.0),
    sec("listening", "Listening", "Summarize spoken text, highlight incorrect words, dictation.", 30.0),
];
const PTE_GROUPS: &[TemplateGroupSpec] = &[
    grp("sw", "voice_record", 6, "Read the text aloud.", "Read Aloud: 40 detik persiapan, lalu rekam."),
    grp("sw", "image_answer", 3, "Describe the image.", "Describe Image: grafik/peta/diagram; 25 detik persiapan."),
    grp("sw", "essay", 1, "Write an essay of 200-300 words.", "Rubrik PTE: content, form, grammar, vocabulary, spelling."),
    grp("reading", "cloze_passage", 10, "Select the correct word for each blank.", "Reading & Writing: Fill in the Blanks dengan dropdown."),
    grp_as("reading", "gap_fill", 10, "ielts", "Drag the words into the blanks.", "Reading: Fill in the Blanks versi drag-and-drop dengan pengecoh."),
    grp("reading", "sentence_reorder", 4, "Restore the original order.", "Re-order Paragraphs."),
    grp("listening", "highlight_incorrect_words", 4, "Click the words that differ from the recording.", "Highlight Incorrect Words: transkrip dengan beberapa kata diganti."),
    grp("listening", "short_answer", 6, "Type the sentence you hear.", "Write from Dictation: ketik persis kalimat yang didengar."),
];

// PTE Core — a shorter, real-life-focused Pearson test (Canadian
// immigration), distinct from PTE Academic above: no essay-length
// writing task, integrated skills instead of separate sub-scores.
const PTE_CORE_SECTIONS: &[TemplateSectionSpec] = &[
    sec("speaking-writing", "Speaking & Writing", "Read aloud, repeat sentence, respond, summarize, dan short-write.", 30.0),
    sec("reading", "Reading", "Fill in the blanks dan multiple choice berbasis teks pendek sehari-hari.", 15.0),
    sec("listening", "Listening", "Summarize spoken text dan multiple choice.", 20.0),
];
const PTE_CORE_GROUPS: &[TemplateGroupSpec] = &[
    grp("speaking-writing", "voice_record", 4, "Read the text aloud.", "Read Aloud: teks pendek kehidupan sehari-hari, bukan akademik."),
    grp("speaking-writing", "voice_record", 4, "Answer the question in one or two sentences.", "Respond to a Situation: skenario praktis (mis. menelepon dokter)."),
    grp("speaking-writing", "short_answer", 4, "Write one sentence summarizing the key point.", "Summarize Written Text (short form): 1 kalimat, bukan paragraf."),
    grp("reading", "cloze_passage", 8, "Select the correct word.", "Fill in the Blanks: teks sehari-hari — email, notifikasi, brosur."),
    grp("reading", "multiple_choice", 6, "Choose the best answer.", "Reading pilihan ganda dari teks pendek."),
    grp("listening", "short_answer", 4, "Write one sentence summarizing what you heard.", "Summarize Spoken Text (short form)."),
    grp("listening", "multiple_choice", 6, "Choose the best answer.", "Listening pilihan ganda dari rekaman percakapan pendek."),
];

// ── Duolingo English Test ───────────────────────────────────────────────

const DET_SECTIONS: &[TemplateSectionSpec] = &[sec("adaptive", "Adaptive Section", "Soal adaptif campuran; durasi total ~45 menit.", 45.0)];
const DET_GROUPS: &[TemplateGroupSpec] = &[
    grp("adaptive", "multiple_choice_multiple", 6, "Select all the real English words.", "Read and Select: campuran kata asli dan kata palsu yang terdengar masuk akal."),
    grp("adaptive", "gap_fill", 8, "Type the missing letters.", "Read and Complete: sebagian huruf tiap kata dihilangkan."),
    grp("adaptive", "short_answer", 6, "Type what you hear.", "Listen and Type: dikte kalimat pendek."),
    grp("adaptive", "voice_record", 4, "Read the sentence aloud.", "Read Aloud / Speak: dinilai pelafalan dan kelancaran."),
    grp("adaptive", "image_answer", 2, "Write about the photo.", "Write About the Photo: minimal satu kalimat deskriptif."),
    grp("adaptive", "essay", 1, "Write about the topic.", "Writing Sample: 3-5 menit, dinilai rubrik."),
    grp("adaptive", "voice_record", 1, "Speak about the topic.", "Speaking Sample: 1-3 menit."),
];

// ── HSK ─────────────────────────────────────────────────────────────────

const HSK1_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tingli", "听力 — Listening", "Audio diputar dua kali. Kosakata HSK 1 (150 kata).", 15.0),
    sec("yuedu", "阅读 — Reading", "Kalimat pendek berpinyin.", 17.0),
];
const HSK1_GROUPS: &[TemplateGroupSpec] = &[
    grp_as("tingli", "matching", 5, "ielts", "看图选择 — cocokkan audio dengan gambar.", "Opsi berupa GAMBAR (`image` pada tiap opsi), bukan teks."),
    grp_as("tingli", "true_false", 5, "grid", "判断对错 — benar atau salah.", "Gunakan `options: [\"对\", \"错\"]` agar sesuai lembar aslinya."),
    grp("tingli", "multiple_choice", 10, "选择正确答案", "Dialog pendek dua baris."),
    grp("yuedu", "matching", 5, "看图配对 — jodohkan kalimat dengan gambar.", "Opsi berupa gambar."),
    grp("yuedu", "multiple_choice", 15, "选择正确答案", "Melengkapi kalimat dan memahami makna kalimat pendek."),
];

const HSK4_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tingli", "听力 — Listening", "Kosakata HSK 4 (1200 kata). Audio diputar sekali.", 30.0),
    sec("yuedu", "阅读 — Reading", "Paragraf pendek dan teks sedang.", 40.0),
    sec("shuxie", "书写 — Writing", "Menyusun kalimat dan mengarang dari gambar.", 25.0),
];
const HSK4_GROUPS: &[TemplateGroupSpec] = &[
    grp_as("tingli", "true_false", 10, "grid", "判断对错", "Gunakan `options: [\"对\", \"错\"]`."),
    grp("tingli", "multiple_choice", 35, "选择正确答案", "Dialog dan monolog sedang."),
    grp_as("yuedu", "gap_fill", 10, "ielts", "选词填空 — pilih kata untuk mengisi rumpang.", "Word bank berisi kata; sebagian pengecoh."),
    grp("yuedu", "sentence_reorder", 10, "排列顺序 — susun urutan kalimat.", "Tiga potongan kalimat yang harus diurutkan menjadi paragraf."),
    grp("yuedu", "multiple_choice", 20, "选择正确答案", "Pemahaman teks pendek."),
    grp("shuxie", "sentence_reorder", 10, "完成句子 — susun menjadi kalimat.", "连词成句: beberapa potongan kata disusun jadi satu kalimat utuh."),
    grp("shuxie", "essay", 5, "看图写句子 — tulis kalimat dari gambar.", "Sertakan gambar dan satu kata wajib pakai."),
];

// ── JLPT ────────────────────────────────────────────────────────────────

const JLPT_SECTIONS: &[TemplateSectionSpec] = &[
    sec("moji", "言語知識（文字・語彙）", "Bacaan kanji, ortografi, dan kosakata kontekstual.", 25.0),
    sec("bunpou", "言語知識（文法）・読解", "Tata bahasa dan pemahaman bacaan.", 50.0),
    sec("choukai", "聴解 — Listening", "Pemahaman tugas, poin utama, dan respons cepat.", 30.0),
];
const JLPT_GROUPS: &[TemplateGroupSpec] = &[
    grp("moji", "multiple_choice", 25, "正しい答えを選びなさい。", "漢字読み, 表記, 文脈規定, 言い換え類義."),
    grp("bunpou", "multiple_choice", 20, "正しい答えを選びなさい。", "文法形式の判断 dan 文章の文法."),
    grp("bunpou", "sentence_reorder", 5, "文を組み立てなさい。", "文の組み立て: susun empat potongan menjadi kalimat benar."),
    grp("bunpou", "multiple_choice", 15, "正しい答えを選びなさい。", "読解: 内容理解 (短文・中文・長文) dan 情報検索."),
    grp("choukai", "multiple_choice", 25, "正しい答えを選びなさい。", "課題理解, ポイント理解, 概要理解, 即時応答."),
];

// ── TOPIK ───────────────────────────────────────────────────────────────

const TOPIK1_SECTIONS: &[TemplateSectionSpec] = &[
    sec("deutgi", "듣기 — Listening", "Audio tingkat pemula, diputar dua kali.", 40.0),
    sec("ilkgi", "읽기 — Reading", "Kalimat dan teks pendek.", 60.0),
];
const TOPIK1_GROUPS: &[TemplateGroupSpec] = &[
    grp("deutgi", "multiple_choice", 30, "알맞은 것을 고르십시오.", "Dialog pendek sehari-hari."),
    grp("ilkgi", "multiple_choice", 40, "알맞은 것을 고르십시오.", "Melengkapi kalimat, memahami iklan/pengumuman, dan teks pendek."),
];

const TOPIK2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("deutgi", "듣기 — Listening", "Audio tingkat menengah-lanjut.", 60.0),
    sec("sseugi", "쓰기 — Writing", "Melengkapi kalimat dan mengarang.", 50.0),
    sec("ilkgi", "읽기 — Reading", "Teks menengah-lanjut.", 70.0),
];
const TOPIK2_GROUPS: &[TemplateGroupSpec] = &[
    grp("deutgi", "multiple_choice", 50, "알맞은 것을 고르십시오.", "Wawancara, berita, ceramah."),
    grp("sseugi", "short_answer", 2, "빈칸에 알맞은 말을 쓰십시오.", "Melengkapi dua rumpang dalam teks pendek."),
    grp("sseugi", "essay", 2, "글을 쓰십시오.", "200-300자 dan 600-700자; dinilai rubrik."),
    grp("ilkgi", "multiple_choice", 50, "알맞은 것을 고르십시오.", "Pemahaman teks panjang, urutan paragraf, dan maksud penulis."),
];

// ── DELF / Goethe ───────────────────────────────────────────────────────

const DELF_SECTIONS: &[TemplateSectionSpec] = &[
    sec("co", "Compréhension de l'oral", "Dokumen audio pendek.", 25.0),
    sec("ce", "Compréhension des écrits", "Dokumen tertulis autentik.", 30.0),
    sec("pe", "Production écrite", "Menulis surat/esai sesuai level.", 45.0),
    sec("po", "Production orale", "Wawancara terarah dan monolog.", 15.0),
];
const DELF_GROUPS: &[TemplateGroupSpec] = &[
    grp("co", "multiple_choice", 12, "Choisissez la bonne réponse.", "Pemahaman audio: informasi utama dan detail."),
    grp("co", "short_answer", 4, "Répondez brièvement.", "Jawaban singkat dari audio."),
    grp("ce", "multiple_choice", 12, "Choisissez la bonne réponse.", "Pemahaman teks autentik."),
    grp_as("ce", "true_false", 5, "grid", "Vrai ou Faux — justifiez.", "Gunakan `options: [\"Vrai\", \"Faux\"]`."),
    grp("pe", "essay", 1, "Rédigez votre texte.", "Panjang sesuai level; dinilai rubrik DELF."),
    grp("po", "voice_record", 3, "Parlez.", "Entretien dirigé, monologue suivi, exercice en interaction."),
];

const GOETHE_SECTIONS: &[TemplateSectionSpec] = &[
    sec("lesen", "Lesen", "Teks autentik berbahasa Jerman.", 45.0),
    sec("hoeren", "Hören", "Audio percakapan dan pengumuman.", 30.0),
    sec("schreiben", "Schreiben", "Menulis surat/esai.", 45.0),
    sec("sprechen", "Sprechen", "Presentasi dan dialog.", 15.0),
];
const GOETHE_GROUPS: &[TemplateGroupSpec] = &[
    grp("lesen", "multiple_choice", 15, "Wählen Sie die richtige Antwort.", "Globalverstehen dan Detailverstehen."),
    grp_as("lesen", "matching", 5, "ielts", "Ordnen Sie zu.", "Mencocokkan teks pendek dengan situasi."),
    grp_as("hoeren", "true_false", 6, "grid", "Richtig oder Falsch?", "Gunakan `options: [\"Richtig\", \"Falsch\"]`."),
    grp("hoeren", "multiple_choice", 10, "Wählen Sie die richtige Antwort.", "Pemahaman audio."),
    grp("schreiben", "essay", 2, "Schreiben Sie.", "Surat/e-mail dan esai pendek; dinilai rubrik."),
    grp("sprechen", "voice_record", 2, "Sprechen Sie.", "Presentasi singkat dan dialog."),
];


// ── SNBT: subtes yang tersisa ───────────────────────────────────────────

const SNBT_PPU_SECTIONS: &[TemplateSectionSpec] = &[sec("ppu", "Pengetahuan dan Pemahaman Umum", "Wawasan umum, kosakata baku, hubungan antarkalimat, dan koherensi paragraf berbahasa Indonesia.", 15.0)];
const SNBT_PPU_GROUPS: &[TemplateGroupSpec] = &[
    grp("ppu", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Sinonim/antonim, kata bentukan, dan hubungan antarkalimat dalam paragraf."),
    grp("ppu", "gap_fill", 5, "Lengkapi bagian rumpang.", "Melengkapi paragraf dengan kata penghubung atau kalimat yang koheren."),
    grp("ppu", "sentence_reorder", 3, "Susun menjadi paragraf padu.", "Beberapa kalimat acak disusun menjadi paragraf yang runtut."),
];

// ── TKA 2025 & TKA lama (utbk-saintek / utbk-soshum) ────────────────────

const TKA_SAINTEK_SECTIONS: &[TemplateSectionSpec] = &[
    sec("mat", "Matematika", "Matematika tingkat lanjut SMA.", 30.0),
    sec("fis", "Fisika", "Mekanika, termodinamika, gelombang, listrik-magnet, fisika modern.", 30.0),
    sec("kim", "Kimia", "Stoikiometri, termokimia, laju reaksi, kesetimbangan, kimia organik.", 30.0),
    sec("bio", "Biologi", "Sel, genetika, evolusi, ekologi, sistem organ.", 30.0),
];
const TKA_SAINTEK_GROUPS: &[TemplateGroupSpec] = &[
    grp("mat", "multiple_choice", 10, "Pilih jawaban yang paling tepat.", "Gunakan LaTeX untuk setiap rumus dan notasi."),
    grp("mat", "short_answer", 5, "Isikan jawaban berupa angka.", "Hasil akhir numerik."),
    grp("fis", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Sertakan soal berbasis grafik/diagram. Gunakan LaTeX dan satuan SI."),
    grp("fis", "short_answer", 3, "Isikan jawaban berupa angka.", "Sertakan satuan pada pertanyaan, bukan pada kunci."),
    grp("kim", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Persamaan reaksi ditulis dengan LaTeX."),
    grp("kim", "short_answer", 3, "Isikan jawaban berupa angka.", "Perhitungan stoikiometri."),
    grp("bio", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Sertakan soal berbasis gambar/diagram proses."),
    grp_as("bio", "true_false", 3, "grid", "Tentukan Benar atau Salah.", "Pernyataan atas satu stimulus proses biologis."),
];

const TKA_SOSHUM_SECTIONS: &[TemplateSectionSpec] = &[
    sec("eko", "Ekonomi", "Mikro, makro, akuntansi dasar.", 30.0),
    sec("geo", "Geografi", "Geografi fisik, manusia, dan penginderaan jauh.", 30.0),
    sec("sej", "Sejarah", "Sejarah Indonesia dan dunia.", 30.0),
    sec("sos", "Sosiologi", "Struktur sosial, perubahan sosial, dan penelitian sosial.", 30.0),
];
const TKA_SOSHUM_GROUPS: &[TemplateGroupSpec] = &[
    grp("eko", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Sertakan soal berbasis kurva dan tabel data ekonomi."),
    grp("eko", "short_answer", 3, "Isikan jawaban berupa angka.", "Perhitungan akuntansi atau elastisitas."),
    grp("geo", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Sertakan soal berbasis peta dan citra."),
    grp("geo", "matching", 3, "Jodohkan.", "Mencocokkan fenomena geografis dengan wilayah atau konsepnya."),
    grp("sej", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Kronologi, kausalitas, dan interpretasi sumber sejarah."),
    grp("sej", "sentence_reorder", 3, "Urutkan peristiwa.", "Susun peristiwa sejarah sesuai urutan waktu."),
    grp("sos", "multiple_choice", 12, "Pilih jawaban yang paling tepat.", "Kasus sosial kontekstual Indonesia."),
    grp_as("sos", "true_false", 3, "grid", "Tentukan Benar atau Salah.", "Pernyataan atas satu kasus sosial."),
];

const TKA_LITERASI_SECTIONS: &[TemplateSectionSpec] = &[sec("teks", "Literasi Bahasa Indonesia", "Teks informasional dan sastra.", 45.0)];
const TKA_LITERASI_GROUPS: &[TemplateGroupSpec] = &[
    grp("teks", "multiple_choice", 20, "Pilih jawaban yang paling tepat.", "Menemukan, menafsirkan, mengevaluasi, dan merefleksi isi teks."),
    grp_as("teks", "true_false", 6, "grid", "Tentukan Benar atau Salah.", "Pernyataan evaluatif atas teks."),
    grp("teks", "multiple_choice_multiple", 4, "Pilih semua yang benar.", "Pilihan ganda kompleks."),
];

const TKA_NUMERASI_SECTIONS: &[TemplateSectionSpec] = &[sec("num", "Numerasi", "Masalah kontekstual berbasis data.", 45.0)];
const TKA_NUMERASI_GROUPS: &[TemplateGroupSpec] = &[
    grp("num", "multiple_choice", 15, "Pilih jawaban yang paling tepat.", "Bilangan, aljabar, geometri-pengukuran, data-ketidakpastian. Gunakan LaTeX."),
    grp("num", "short_answer", 6, "Isikan jawaban berupa angka.", "Terima desimal koma maupun titik."),
    grp("num", "table_completion", 3, "Lengkapi tabel.", "Melengkapi tabel dari hasil perhitungan."),
    grp_as("num", "true_false", 6, "grid", "Tentukan Benar atau Salah.", "Pernyataan atas hasil pembacaan data."),
];

const TKA_INGGRIS_SECTIONS: &[TemplateSectionSpec] = &[sec("eng", "Bahasa Inggris", "Teks informasional level B1-B2.", 30.0)];
const TKA_INGGRIS_GROUPS: &[TemplateGroupSpec] = &[
    grp("eng", "multiple_choice", 15, "Choose the best answer.", "Main idea, detail, inference, vocabulary in context."),
    grp("eng", "true_false_not_given", 5, "Do the statements agree with the passage?", "True / False / Not Given."),
    grp("eng", "gap_fill", 5, "Complete the text.", "Melengkapi teks rumpang dengan kata yang tepat."),
];

// ── Mandiri PTN, UM-PTKIN, Psikotes ─────────────────────────────────────

const MANDIRI_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tpa", "Tes Potensi Akademik", "Verbal, numerik, dan penalaran figural.", 45.0),
    sec("bidang", "Tes Mata Uji", "Mata pelajaran sesuai kelompok pilihan.", 60.0),
];
const MANDIRI_GROUPS: &[TemplateGroupSpec] = &[
    grp("tpa", "analogy", 5, "Pilih pasangan yang hubungannya paling serupa.", "Analogi verbal setara TPA."),
    grp("tpa", "multiple_choice", 25, "Pilih jawaban yang paling tepat.", "Sinonim/antonim, silogisme, deret angka, dan penalaran figural."),
    grp("tpa", "short_answer", 5, "Isikan jawaban berupa angka.", "Hitung cepat dan pola bilangan."),
    grp("bidang", "multiple_choice", 30, "Pilih jawaban yang paling tepat.", "Mata uji sesuai kelompok (Saintek atau Soshum). Gunakan LaTeX bila perlu."),
    grp("bidang", "short_answer", 5, "Isikan jawaban.", "Jawaban singkat berupa angka atau istilah."),
];

const UMPTKIN_SECTIONS: &[TemplateSectionSpec] = &[
    sec("pa", "Penalaran Akademik", "Penalaran verbal, numerik, dan figural.", 30.0),
    sec("litid", "Literasi Bahasa Indonesia", "Teks informasional berbahasa Indonesia.", 25.0),
    sec("liten", "Literasi Bahasa Inggris", "Teks informasional berbahasa Inggris.", 20.0),
    sec("litar", "Literasi Bahasa Arab", "Teks berbahasa Arab sesuai jenjang madrasah.", 20.0),
    sec("keislaman", "Penalaran Keislaman", "Al-Qur'an Hadis, Akidah Akhlak, Fikih, SKI.", 30.0),
];
const UMPTKIN_GROUPS: &[TemplateGroupSpec] = &[
    grp("pa", "multiple_choice", 20, "Pilih jawaban yang paling tepat.", "Penalaran verbal, numerik, dan figural."),
    grp("litid", "multiple_choice", 15, "Pilih jawaban yang paling tepat.", "Pemahaman dan evaluasi teks."),
    grp("liten", "multiple_choice", 15, "Choose the best answer.", "Reading comprehension level B1."),
    grp("litar", "multiple_choice", 15, "اختر الإجابة الصحيحة.", "Teks Arab dengan harakat sesuai jenjang. Tulis soal dan opsi dalam aksara Arab."),
    grp("keislaman", "multiple_choice", 20, "Pilih jawaban yang paling tepat.", "Al-Qur'an Hadis, Akidah Akhlak, Fikih, dan Sejarah Kebudayaan Islam."),
];

const PSIKOTES_SECTIONS: &[TemplateSectionSpec] = &[
    sec("kognitif", "Tes Kognitif", "Verbal, numerik, logika, dan figural.", 45.0),
    sec("kepribadian", "Tes Kepribadian", "Inventori sikap kerja; tidak ada jawaban benar-salah.", 20.0),
];
const PSIKOTES_GROUPS: &[TemplateGroupSpec] = &[
    grp("kognitif", "analogy", 10, "Pilih pasangan yang hubungannya paling serupa.", "Analogi verbal."),
    grp("kognitif", "multiple_choice", 15, "Pilih jawaban yang paling tepat.", "Deret angka, deret huruf, dan silogisme."),
    grp("kognitif", "image_word", 10, "Pilih gambar yang tepat.", "Penalaran figural: rotasi, pencerminan, dan kelanjutan pola. Opsi berupa GAMBAR."),
    grp("kepribadian", "likert_scale", 20, "Seberapa sesuai pernyataan ini dengan Anda?", "Inventori sikap kerja, skala 1-5. Tidak dinilai benar-salah."),
    grp("kepribadian", "multiple_choice", 10, "Pilih yang paling menggambarkan Anda.", "WAJIB: `option_scores` 1-5 per opsi. Situasi kerja nyata."),
];

// Kraepelin/Pauli — deret penjumlahan angka tunggal berurutan, dikerjakan
// secepat mungkin; tiap baris/kolom sebenarnya satu soal berkelanjutan,
// direpresentasikan di sini sebagai banyak soal short_answer pendek
// (jumlah dua digit berurutan) supaya tetap auto-gradable.
const KRAEPELIN_SECTIONS: &[TemplateSectionSpec] = &[
    sec("kolom", "Deret Penjumlahan", "Jumlahkan tiap dua angka berurutan secepat dan setepat mungkin.", 20.0),
];
const KRAEPELIN_GROUPS: &[TemplateGroupSpec] = &[
    grp(
        "kolom",
        "short_answer",
        40,
        "Jumlahkan angka ini dengan angka sebelumnya.",
        "Tiap soal: dua digit acak (0-9) yang dijumlahkan dengan digit pada soal sebelumnya, jawaban satu angka (0-18) atau angka satuan jika hasil dua digit — ikuti konvensi Kraepelin (ambil digit satuan). Soal berurutan membentuk satu kolom; sajikan berkelompok 20 soal per 'kolom' dengan instruksi ganti kolom di tengah.",
    ),
];

// DISC — 24 blok forced-choice, tiap blok 4 pernyataan (D/I/S/C); peserta
// memilih SATU pernyataan paling menggambarkan dan SATU paling tidak
// menggambarkan dirinya. Direpresentasikan sebagai grup multiple_choice
// dengan option_scores yang memetakan tiap opsi ke satu dari 4 dimensi.
const DISC_SECTIONS: &[TemplateSectionSpec] = &[
    sec("profil", "Profil Perilaku DISC", "24 blok pernyataan; pilih yang paling menggambarkan Anda.", 20.0),
];
const DISC_GROUPS: &[TemplateGroupSpec] = &[
    grp(
        "profil",
        "multiple_choice",
        24,
        "Pilih pernyataan yang PALING menggambarkan diri Anda.",
        "WAJIB: 4 opsi per soal, masing-masing mewakili satu dimensi Dominance/Influence/Steadiness/Compliance. Gunakan `option_scores` untuk menandai dimensi tiap opsi (mis. {\"D\":1} dsb, bukan skor benar-salah). Tidak ada jawaban benar-salah.",
    ),
];

// ── CPNS SKB & BUMN Learning Agility ────────────────────────────────────

const CPNS_SKB_SECTIONS: &[TemplateSectionSpec] = &[sec("skb", "Seleksi Kompetensi Bidang", "Kompetensi teknis sesuai formasi jabatan yang dilamar.", 60.0)];
const CPNS_SKB_GROUPS: &[TemplateGroupSpec] = &[
    // Split across two groups — a single "Generate soal" call is capped
    // at 50 questions (generate_quiz_group::MAX_COUNT), so 80 in one
    // group could never actually be AI-generated at all.
    grp("skb", "multiple_choice", 40, "Pilih jawaban yang benar.", "Kompetensi teknis jabatan: regulasi dan penerapan prosedur kerja."),
    grp("skb", "multiple_choice", 40, "Pilih jawaban yang benar.", "Kompetensi teknis jabatan: studi kasus penerapan di lapangan."),
    grp("skb", "essay", 2, "Uraikan jawaban Anda.", "Studi kasus jabatan; dinilai rubrik."),
];

const BUMN_AGILITY_SECTIONS: &[TemplateSectionSpec] = &[sec("agility", "Learning Agility", "Kemampuan belajar dan beradaptasi pada situasi baru.", 30.0)];
const BUMN_AGILITY_GROUPS: &[TemplateGroupSpec] = &[
    grp("agility", "multiple_choice", 20, "Pilih tindakan yang paling menggambarkan Anda.", "WAJIB: `option_scores` 1-5. Situasi perubahan, ambiguitas, dan pembelajaran cepat."),
    grp("agility", "sentence_reorder", 5, "Susun langkah yang paling masuk akal.", "Mengurutkan langkah penyelesaian masalah baru."),
    grp("agility", "image_word", 5, "Pilih pola yang tepat.", "Pengenalan pola abstrak. Opsi berupa GAMBAR."),
];

// ── OSN per bidang ──────────────────────────────────────────────────────

const OSN_MIPA_SECTIONS: &[TemplateSectionSpec] = &[
    sec("isian", "Isian Singkat", "Jawaban tunggal dan pasti.", 90.0),
    sec("uraian", "Uraian", "Pembuktian dan pemecahan masalah bertahap.", 120.0),
];
const OSN_MAT_GROUPS: &[TemplateGroupSpec] = &[
    grp("isian", "short_answer", 12, "Tuliskan jawaban akhir.", "Aljabar, kombinatorika, teori bilangan, geometri setingkat OSN. Gunakan LaTeX."),
    grp("uraian", "essay", 4, "Tuliskan penyelesaian lengkap.", "Soal pembuktian; dinilai rubrik per langkah."),
];
const OSN_FIS_GROUPS: &[TemplateGroupSpec] = &[
    grp("isian", "short_answer", 10, "Tuliskan jawaban akhir beserta satuan.", "Mekanika, termodinamika, elektromagnetik, fisika modern. Gunakan LaTeX dan satuan SI."),
    grp("uraian", "essay", 4, "Tuliskan penurunan lengkap.", "Penurunan rumus dan analisis; dinilai rubrik."),
];
const OSN_KIM_GROUPS: &[TemplateGroupSpec] = &[
    grp("isian", "short_answer", 10, "Tuliskan jawaban akhir.", "Stoikiometri, termokimia, kesetimbangan, kimia organik. Persamaan reaksi dengan LaTeX."),
    grp("uraian", "essay", 4, "Tuliskan penyelesaian lengkap.", "Mekanisme reaksi dan perhitungan bertahap; dinilai rubrik."),
];
const OSN_BIO_SECTIONS: &[TemplateSectionSpec] = &[
    sec("pilihan", "Pilihan Ganda", "Cakupan biologi sel sampai ekologi.", 90.0),
    sec("analisis", "Analisis Data", "Interpretasi hasil percobaan.", 60.0),
];
const OSN_BIO_GROUPS: &[TemplateGroupSpec] = &[
    grp("pilihan", "multiple_choice", 40, "Pilih jawaban yang benar.", "Sel, genetika, fisiologi, evolusi, ekologi, dan biosistematika."),
    grp_as("pilihan", "true_false", 10, "grid", "Tentukan Benar atau Salah.", "Pernyataan atas satu kasus biologis."),
    grp("analisis", "essay", 3, "Analisis data berikut.", "Interpretasi grafik/tabel hasil percobaan; dinilai rubrik."),
];
const OSN_EKO_SECTIONS: &[TemplateSectionSpec] = &[
    sec("teori", "Teori Ekonomi", "Mikro, makro, dan ekonomi internasional.", 90.0),
    sec("kasus", "Studi Kasus", "Analisis kebijakan dan data ekonomi.", 60.0),
];
const OSN_EKO_GROUPS: &[TemplateGroupSpec] = &[
    grp("teori", "multiple_choice", 40, "Pilih jawaban yang benar.", "Mikroekonomi, makroekonomi, dan ekonomi internasional setingkat OSN."),
    grp("teori", "short_answer", 10, "Isikan jawaban berupa angka.", "Perhitungan elastisitas, PDB, dan indeks."),
    grp("kasus", "essay", 3, "Analisis kasus berikut.", "Analisis kebijakan berbasis data; dinilai rubrik."),
];
const OSN_GEO_SECTIONS: &[TemplateSectionSpec] = &[
    sec("teori", "Teori Geografi", "Geosfer, penginderaan jauh, dan SIG.", 90.0),
    sec("praktik", "Analisis Peta & Data", "Membaca peta, citra, dan data spasial.", 60.0),
];
const OSN_GEO_GROUPS: &[TemplateGroupSpec] = &[
    grp("teori", "multiple_choice", 40, "Pilih jawaban yang benar.", "Atmosfer, litosfer, hidrosfer, biosfer, geografi manusia, penginderaan jauh, dan SIG."),
    grp_as("teori", "map_labeling", 5, "ielts", "Beri label pada peta.", "Peta atau citra dengan pool label."),
    grp("praktik", "essay", 3, "Analisis peta berikut.", "Interpretasi peta/citra; dinilai rubrik."),
];

// ── HSK 2, 3, 5, 6 ──────────────────────────────────────────────────────

const HSK2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tingli", "听力 — Listening", "Kosakata HSK 2 (300 kata). Audio diputar dua kali.", 25.0),
    sec("yuedu", "阅读 — Reading", "Kalimat pendek berpinyin.", 22.0),
];
const HSK2_GROUPS: &[TemplateGroupSpec] = &[
    grp_as("tingli", "matching", 10, "ielts", "看图选择 — cocokkan audio dengan gambar.", "Opsi berupa GAMBAR (`image` pada tiap opsi)."),
    grp_as("tingli", "true_false", 10, "grid", "判断对错", "Gunakan `options: [\"对\", \"错\"]`."),
    grp("tingli", "multiple_choice", 15, "选择正确答案", "Dialog pendek dua sampai tiga baris."),
    grp_as("yuedu", "matching", 10, "ielts", "配对 — jodohkan kalimat dengan gambar atau kalimat lain.", "Sebagian opsi berupa gambar."),
    grp_as("yuedu", "gap_fill", 5, "ielts", "选词填空", "Word bank berisi kata HSK 2."),
    grp("yuedu", "multiple_choice", 10, "选择正确答案", "Memahami kalimat pendek."),
];

const HSK3_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tingli", "听力 — Listening", "Kosakata HSK 3 (600 kata). Audio diputar dua kali.", 35.0),
    sec("yuedu", "阅读 — Reading", "Paragraf pendek.", 30.0),
    sec("shuxie", "书写 — Writing", "Menyusun kalimat dan menulis karakter.", 15.0),
];
const HSK3_GROUPS: &[TemplateGroupSpec] = &[
    grp_as("tingli", "matching", 10, "ielts", "看图选择", "Opsi berupa GAMBAR."),
    grp_as("tingli", "true_false", 10, "grid", "判断对错", "Gunakan `options: [\"对\", \"错\"]`."),
    grp("tingli", "multiple_choice", 20, "选择正确答案", "Dialog dan monolog pendek."),
    grp_as("yuedu", "matching", 10, "ielts", "配对", "Mencocokkan kalimat dengan tanggapannya."),
    grp_as("yuedu", "gap_fill", 10, "ielts", "选词填空", "Word bank berisi kata HSK 3 dengan pengecoh."),
    grp("yuedu", "multiple_choice", 10, "选择正确答案", "Pemahaman paragraf pendek."),
    grp("shuxie", "sentence_reorder", 5, "完成句子 — susun menjadi kalimat.", "连词成句: potongan kata disusun jadi kalimat utuh."),
    grp("shuxie", "short_answer", 5, "填空 — tulis karakter yang tepat.", "Menulis satu karakter Han sesuai pinyin dan konteks kalimat."),
];

const HSK5_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tingli", "听力 — Listening", "Kosakata HSK 5 (2500 kata). Audio diputar sekali.", 30.0),
    sec("yuedu", "阅读 — Reading", "Teks sedang sampai panjang.", 45.0),
    sec("shuxie", "书写 — Writing", "Menyusun kalimat dan mengarang.", 40.0),
];
const HSK5_GROUPS: &[TemplateGroupSpec] = &[
    grp("tingli", "multiple_choice", 45, "选择正确答案", "Dialog panjang, wawancara, dan monolog."),
    grp_as("yuedu", "gap_fill", 15, "ielts", "选词填空", "Melengkapi paragraf dengan kata dari word bank."),
    grp("yuedu", "multiple_choice", 30, "选择正确答案", "Pemahaman teks panjang dan maksud penulis."),
    grp("shuxie", "sentence_reorder", 8, "完成句子", "连词成句 tingkat lanjut."),
    grp("shuxie", "essay", 2, "写短文 — tulis karangan.", "Karangan 80 karakter dari kata kunci atau gambar; dinilai rubrik."),
];

const HSK6_SECTIONS: &[TemplateSectionSpec] = &[
    sec("tingli", "听力 — Listening", "Kosakata HSK 6 (5000 kata). Audio diputar sekali.", 35.0),
    sec("yuedu", "阅读 — Reading", "Teks panjang dan padat.", 50.0),
    sec("shuxie", "书写 — Writing", "Meringkas teks panjang.", 45.0),
];
const HSK6_GROUPS: &[TemplateGroupSpec] = &[
    grp("tingli", "multiple_choice", 50, "选择正确答案", "Berita, wawancara, dan ceramah."),
    grp("yuedu", "multiple_choice", 40, "选择正确答案", "Pemahaman teks panjang, idiom, dan kalimat yang salah."),
    grp_as("yuedu", "gap_fill", 10, "ielts", "选词填空", "Melengkapi paragraf dengan frasa dari word bank."),
    grp("shuxie", "essay", 1, "缩写 — ringkas teks berikut.", "Membaca teks ~1000 karakter lalu meringkas jadi ~400 karakter; dinilai rubrik."),
];

// ── JLPT N5, N4, N2, N1 ─────────────────────────────────────────────────

const JLPT_N5_SECTIONS: &[TemplateSectionSpec] = &[
    sec("moji", "言語知識（文字・語彙）", "Hiragana, katakana, dan kanji dasar.", 20.0),
    sec("bunpou", "言語知識（文法）・読解", "Tata bahasa dasar dan bacaan pendek.", 40.0),
    sec("choukai", "聴解 — Listening", "Percakapan sehari-hari sederhana.", 30.0),
];
const JLPT_N5_GROUPS: &[TemplateGroupSpec] = &[
    grp("moji", "multiple_choice", 20, "正しい答えを選びなさい。", "漢字読み, 表記, 文脈規定, 言い換え類義 tingkat N5."),
    grp("bunpou", "multiple_choice", 15, "正しい答えを選びなさい。", "文法形式の判断 tingkat N5."),
    grp("bunpou", "sentence_reorder", 4, "文を組み立てなさい。", "文の組み立て: susun empat potongan menjadi kalimat."),
    grp("bunpou", "multiple_choice", 8, "正しい答えを選びなさい。", "読解: teks pendek dan pencarian informasi."),
    grp("choukai", "multiple_choice", 20, "正しい答えを選びなさい。", "課題理解, ポイント理解, 発話表現, 即時応答."),
];

const JLPT_N4_SECTIONS: &[TemplateSectionSpec] = &[
    sec("moji", "言語知識（文字・語彙）", "Kanji dan kosakata tingkat N4.", 25.0),
    sec("bunpou", "言語知識（文法）・読解", "Tata bahasa dan bacaan tingkat N4.", 55.0),
    sec("choukai", "聴解 — Listening", "Percakapan sehari-hari.", 35.0),
];
const JLPT_N4_GROUPS: &[TemplateGroupSpec] = &[
    grp("moji", "multiple_choice", 22, "正しい答えを選びなさい。", "漢字読み, 表記, 文脈規定, 言い換え類義, 用法 tingkat N4."),
    grp("bunpou", "multiple_choice", 18, "正しい答えを選びなさい。", "文法形式の判断 dan 文章の文法."),
    grp("bunpou", "sentence_reorder", 4, "文を組み立てなさい。", "文の組み立て."),
    grp("bunpou", "multiple_choice", 10, "正しい答えを選びなさい。", "読解: 短文・中文 dan 情報検索."),
    grp("choukai", "multiple_choice", 24, "正しい答えを選びなさい。", "課題理解, ポイント理解, 発話表現, 即時応答."),
];

const JLPT_N2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("goi", "言語知識・読解", "Kosakata, tata bahasa, dan bacaan tingkat N2.", 105.0),
    sec("choukai", "聴解 — Listening", "Berita, diskusi, dan ceramah.", 50.0),
];
const JLPT_N2_GROUPS: &[TemplateGroupSpec] = &[
    grp("goi", "multiple_choice", 30, "正しい答えを選びなさい。", "文字・語彙: 漢字読み, 表記, 語形成, 文脈規定, 言い換え類義, 用法."),
    grp("goi", "multiple_choice", 12, "正しい答えを選びなさい。", "文法形式の判断 dan 文章の文法 tingkat N2."),
    grp("goi", "sentence_reorder", 5, "文を組み立てなさい。", "文の組み立て tingkat N2."),
    grp("goi", "multiple_choice", 20, "正しい答えを選びなさい。", "読解: 短文・中文・長文, 統合理解, 主張理解, 情報検索."),
    grp("choukai", "multiple_choice", 32, "正しい答えを選びなさい。", "課題理解, ポイント理解, 概要理解, 即時応答, 統合理解."),
];

const JLPT_N1_SECTIONS: &[TemplateSectionSpec] = &[
    sec("goi", "言語知識・読解", "Kosakata, tata bahasa, dan bacaan tingkat N1.", 110.0),
    sec("choukai", "聴解 — Listening", "Ceramah akademik dan diskusi kompleks.", 60.0),
];
const JLPT_N1_GROUPS: &[TemplateGroupSpec] = &[
    grp("goi", "multiple_choice", 25, "正しい答えを選びなさい。", "文字・語彙 tingkat N1."),
    grp("goi", "multiple_choice", 15, "正しい答えを選びなさい。", "文法 tingkat N1."),
    grp("goi", "sentence_reorder", 5, "文を組み立てなさい。", "文の組み立て tingkat N1."),
    grp("goi", "multiple_choice", 25, "正しい答えを選びなさい。", "読解: teks panjang, argumentatif, dan perbandingan sumber."),
    grp("choukai", "multiple_choice", 35, "正しい答えを選びなさい。", "課題理解, ポイント理解, 概要理解, 即時応答, 統合理解."),
];

// ── DELF A1, A2, B2 ─────────────────────────────────────────────────────

const DELF_A1_SECTIONS: &[TemplateSectionSpec] = &[
    sec("co", "Compréhension de l'oral", "Dokumen audio sangat pendek dan sederhana.", 20.0),
    sec("ce", "Compréhension des écrits", "Dokumen tertulis sederhana.", 30.0),
    sec("pe", "Production écrite", "Mengisi formulir dan menulis pesan pendek.", 30.0),
    sec("po", "Production orale", "Perkenalan diri dan dialog sederhana.", 10.0),
];
const DELF_A1_GROUPS: &[TemplateGroupSpec] = &[
    grp("co", "multiple_choice", 8, "Choisissez la bonne réponse.", "Angka, waktu, harga, dan instruksi sederhana."),
    grp("co", "short_answer", 4, "Répondez brièvement.", "Jawaban satu sampai dua kata."),
    grp("ce", "multiple_choice", 8, "Choisissez la bonne réponse.", "Pengumuman, jadwal, dan pesan pendek."),
    grp_as("ce", "true_false", 4, "grid", "Vrai ou Faux ?", "Gunakan `options: [\"Vrai\", \"Faux\"]`."),
    grp("pe", "essay", 2, "Rédigez.", "Mengisi formulir dan menulis kartu pos ~40 kata; dinilai rubrik."),
    grp("po", "voice_record", 3, "Parlez.", "Entretien dirigé, échange d'informations, dialogue simulé."),
];

const DELF_A2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("co", "Compréhension de l'oral", "Dokumen audio pendek sehari-hari.", 25.0),
    sec("ce", "Compréhension des écrits", "Dokumen tertulis sehari-hari.", 30.0),
    sec("pe", "Production écrite", "Menulis surat dan menceritakan pengalaman.", 45.0),
    sec("po", "Production orale", "Monolog dan dialog terarah.", 12.0),
];
const DELF_A2_GROUPS: &[TemplateGroupSpec] = &[
    grp("co", "multiple_choice", 10, "Choisissez la bonne réponse.", "Pengumuman, pesan telepon, dan percakapan pendek."),
    grp("co", "short_answer", 4, "Répondez brièvement.", "Detail dari audio."),
    grp("ce", "multiple_choice", 10, "Choisissez la bonne réponse.", "Surat, iklan, dan artikel pendek."),
    grp_as("ce", "true_false", 5, "grid", "Vrai ou Faux ?", "Gunakan `options: [\"Vrai\", \"Faux\"]`."),
    grp("pe", "essay", 2, "Rédigez.", "Menceritakan pengalaman dan menulis undangan ~60-80 kata; dinilai rubrik."),
    grp("po", "voice_record", 3, "Parlez.", "Entretien dirigé, monologue suivi, exercice en interaction."),
];

const DELF_B2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("co", "Compréhension de l'oral", "Wawancara, berita radio, dan ceramah.", 30.0),
    sec("ce", "Compréhension des écrits", "Teks informatif dan argumentatif.", 60.0),
    sec("pe", "Production écrite", "Esai argumentatif.", 60.0),
    sec("po", "Production orale", "Presentasi berargumen dan debat.", 20.0),
];
const DELF_B2_GROUPS: &[TemplateGroupSpec] = &[
    grp("co", "multiple_choice", 12, "Choisissez la bonne réponse.", "Wawancara dan berita; pemahaman detail dan sikap penutur."),
    grp("co", "short_answer", 5, "Répondez.", "Jawaban singkat dari audio panjang."),
    grp("ce", "multiple_choice", 12, "Choisissez la bonne réponse.", "Teks argumentatif; maksud penulis dan struktur argumen."),
    grp_as("ce", "true_false", 5, "grid", "Vrai ou Faux — justifiez.", "Gunakan `options: [\"Vrai\", \"Faux\"]`."),
    grp("pe", "essay", 1, "Rédigez un texte argumenté.", "Esai argumentatif ~250 kata; dinilai rubrik DELF B2."),
    grp("po", "voice_record", 2, "Présentez et débattez.", "Presentasi berdasarkan artikel, lalu debat dengan penguji."),
];

// ── Goethe A1, A2, B2 ───────────────────────────────────────────────────

const GOETHE_A1_SECTIONS: &[TemplateSectionSpec] = &[
    sec("lesen", "Lesen", "Teks sangat pendek dan sederhana.", 25.0),
    sec("hoeren", "Hören", "Pengumuman dan percakapan singkat.", 20.0),
    sec("schreiben", "Schreiben", "Mengisi formulir dan menulis pesan.", 20.0),
    sec("sprechen", "Sprechen", "Perkenalan dan permintaan sederhana.", 15.0),
];
const GOETHE_A1_GROUPS: &[TemplateGroupSpec] = &[
    grp("lesen", "multiple_choice", 10, "Wählen Sie die richtige Antwort.", "Pesan pendek, iklan, dan pengumuman."),
    grp_as("lesen", "true_false", 5, "grid", "Richtig oder Falsch?", "Gunakan `options: [\"Richtig\", \"Falsch\"]`."),
    grp_as("hoeren", "matching", 5, "ielts", "Ordnen Sie zu.", "Mencocokkan audio dengan gambar atau situasi."),
    grp("hoeren", "multiple_choice", 10, "Wählen Sie die richtige Antwort.", "Angka, waktu, dan instruksi sederhana."),
    grp("schreiben", "essay", 2, "Schreiben Sie.", "Mengisi formulir dan menulis pesan ~30 kata; dinilai rubrik."),
    grp("sprechen", "voice_record", 3, "Sprechen Sie.", "Perkenalan diri, bertanya, dan meminta sesuatu."),
];

const GOETHE_A2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("lesen", "Lesen", "Teks pendek sehari-hari.", 30.0),
    sec("hoeren", "Hören", "Percakapan dan pengumuman sehari-hari.", 30.0),
    sec("schreiben", "Schreiben", "Menulis pesan dan surat pendek.", 30.0),
    sec("sprechen", "Sprechen", "Bercerita dan merencanakan bersama.", 15.0),
];
const GOETHE_A2_GROUPS: &[TemplateGroupSpec] = &[
    grp("lesen", "multiple_choice", 12, "Wählen Sie die richtige Antwort.", "E-mail, iklan, dan artikel pendek."),
    grp_as("lesen", "matching", 5, "ielts", "Ordnen Sie zu.", "Mencocokkan teks pendek dengan situasi."),
    grp_as("hoeren", "true_false", 5, "grid", "Richtig oder Falsch?", "Gunakan `options: [\"Richtig\", \"Falsch\"]`."),
    grp("hoeren", "multiple_choice", 12, "Wählen Sie die richtige Antwort.", "Percakapan sehari-hari dan pengumuman."),
    grp("schreiben", "essay", 2, "Schreiben Sie.", "Pesan singkat dan surat ~40 kata; dinilai rubrik."),
    grp("sprechen", "voice_record", 3, "Sprechen Sie.", "Bercerita tentang diri, bertanya, dan merencanakan bersama."),
];

const GOETHE_B2_SECTIONS: &[TemplateSectionSpec] = &[
    sec("lesen", "Lesen", "Teks jurnalistik dan argumentatif.", 65.0),
    sec("hoeren", "Hören", "Wawancara, diskusi, dan ceramah.", 40.0),
    sec("schreiben", "Schreiben", "Esai argumentatif dan surat formal.", 75.0),
    sec("sprechen", "Sprechen", "Presentasi dan diskusi.", 15.0),
];
const GOETHE_B2_GROUPS: &[TemplateGroupSpec] = &[
    grp("lesen", "multiple_choice", 18, "Wählen Sie die richtige Antwort.", "Teks jurnalistik; sikap penulis dan struktur argumen."),
    grp_as("lesen", "matching", 6, "ielts", "Ordnen Sie zu.", "Mencocokkan pendapat dengan penulisnya."),
    grp("hoeren", "multiple_choice", 15, "Wählen Sie die richtige Antwort.", "Wawancara dan diskusi; detail dan sikap penutur."),
    grp_as("hoeren", "true_false", 5, "grid", "Richtig oder Falsch?", "Gunakan `options: [\"Richtig\", \"Falsch\"]`."),
    grp("schreiben", "essay", 2, "Schreiben Sie.", "Esai argumentatif ~150 kata dan surat formal; dinilai rubrik."),
    grp("sprechen", "voice_record", 2, "Sprechen Sie.", "Presentasi singkat lalu diskusi dengan mitra."),
];

pub const TEMPLATES: &[QuizTemplate] = &[
    QuizTemplate { id: "snbt_pu", folder: "snbt", subfolder: "Penalaran Umum", name: "SNBT · Penalaran Umum", description: "30 soal · 30 menit. Penalaran induktif, deduktif, dan kuantitatif.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(1800.0), passing_score: Some(60.0), sections: SNBT_PU_SECTIONS, groups: SNBT_PU_GROUPS },
    QuizTemplate { id: "snbt_pbm", folder: "snbt", subfolder: "Pemahaman Bacaan & Menulis", name: "SNBT · Pemahaman Bacaan dan Menulis", description: "20 soal · 25 menit.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(1500.0), passing_score: Some(60.0), sections: SNBT_PBM_SECTIONS, groups: SNBT_PBM_GROUPS },
    QuizTemplate { id: "snbt_pk", folder: "snbt", subfolder: "Pengetahuan Kuantitatif", name: "SNBT · Pengetahuan Kuantitatif", description: "20 soal · 20 menit.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(1200.0), passing_score: Some(60.0), sections: SNBT_PK_SECTIONS, groups: SNBT_PK_GROUPS },
    QuizTemplate { id: "snbt_literasi_id", folder: "snbt", subfolder: "Literasi", name: "SNBT · Literasi Bahasa Indonesia", description: "30 soal · 42,5 menit.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(2550.0), passing_score: Some(60.0), sections: SNBT_LITID_SECTIONS, groups: SNBT_LITID_GROUPS },
    QuizTemplate { id: "snbt_literasi_en", folder: "snbt", subfolder: "Literasi", name: "SNBT · Literasi Bahasa Inggris", description: "20 soal · 22,5 menit.", language: "en", theme: "default", timer_mode: "section", max_duration_seconds: Some(1350.0), passing_score: Some(60.0), sections: SNBT_LITEN_SECTIONS, groups: SNBT_LITEN_GROUPS },
    QuizTemplate { id: "snbt_pm", folder: "snbt", subfolder: "Penalaran Matematika", name: "SNBT · Penalaran Matematika", description: "20 soal · 42,5 menit.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(2550.0), passing_score: Some(60.0), sections: SNBT_PM_SECTIONS, groups: SNBT_PM_GROUPS },

    QuizTemplate { id: "snbt_ppu", folder: "snbt", subfolder: "Pengetahuan & Pemahaman Umum", name: "SNBT · Pengetahuan dan Pemahaman Umum", description: "20 soal · 15 menit.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(900.0), passing_score: Some(60.0), sections: SNBT_PPU_SECTIONS, groups: SNBT_PPU_GROUPS },

    QuizTemplate { id: "tka_saintek", folder: "tka", subfolder: "Saintek", name: "TKA Saintek", description: "Matematika, Fisika, Kimia, Biologi.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(7200.0), passing_score: Some(60.0), sections: TKA_SAINTEK_SECTIONS, groups: TKA_SAINTEK_GROUPS },
    QuizTemplate { id: "tka_soshum", folder: "tka", subfolder: "Soshum", name: "TKA Soshum", description: "Ekonomi, Geografi, Sejarah, Sosiologi.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(7200.0), passing_score: Some(60.0), sections: TKA_SOSHUM_SECTIONS, groups: TKA_SOSHUM_GROUPS },
    QuizTemplate { id: "tka_literasi", folder: "tka", subfolder: "Literasi", name: "TKA Literasi Bahasa Indonesia", description: "30 soal berbasis teks.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(2700.0), passing_score: Some(60.0), sections: TKA_LITERASI_SECTIONS, groups: TKA_LITERASI_GROUPS },
    QuizTemplate { id: "tka_numerasi", folder: "tka", subfolder: "Numerasi", name: "TKA Numerasi", description: "30 soal berbasis data.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(2700.0), passing_score: Some(60.0), sections: TKA_NUMERASI_SECTIONS, groups: TKA_NUMERASI_GROUPS },
    QuizTemplate { id: "tka_inggris", folder: "tka", subfolder: "Bahasa Inggris", name: "TKA Bahasa Inggris", description: "25 soal · level B1-B2.", language: "en", theme: "default", timer_mode: "section", max_duration_seconds: Some(1800.0), passing_score: Some(60.0), sections: TKA_INGGRIS_SECTIONS, groups: TKA_INGGRIS_GROUPS },

    QuizTemplate { id: "mandiri_ptn", folder: "mandiri", subfolder: "Umum", name: "Mandiri PTN · TPA + Mata Uji", description: "Tes Potensi Akademik dan mata uji sesuai kelompok.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(6300.0), passing_score: Some(60.0), sections: MANDIRI_SECTIONS, groups: MANDIRI_GROUPS },
    QuizTemplate { id: "um_ptkin", folder: "umptkin", subfolder: "SSE", name: "UM-PTKIN", description: "Penalaran Akademik, literasi Indonesia/Inggris/Arab, dan keislaman.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(6900.0), passing_score: Some(60.0), sections: UMPTKIN_SECTIONS, groups: UMPTKIN_GROUPS },
    QuizTemplate { id: "psikotes_kerja", folder: "psikotes", subfolder: "Seleksi Kerja", name: "Psikotes · Kognitif + Kepribadian", description: "Verbal, numerik, figural, dan inventori kepribadian berbobot.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(3900.0), passing_score: Some(55.0), sections: PSIKOTES_SECTIONS, groups: PSIKOTES_GROUPS },
    QuizTemplate { id: "psikotes_kraepelin", folder: "psikotes", subfolder: "Kraepelin/Pauli", name: "Psikotes · Kraepelin/Pauli", description: "Deret penjumlahan angka cepat, mengukur ketelitian dan stamina kerja.", language: "id", theme: "default", timer_mode: "global", max_duration_seconds: Some(1200.0), passing_score: Some(50.0), sections: KRAEPELIN_SECTIONS, groups: KRAEPELIN_GROUPS },
    QuizTemplate { id: "psikotes_disc", folder: "psikotes", subfolder: "DISC", name: "Psikotes · DISC", description: "24 blok forced-choice, memetakan profil Dominance/Influence/Steadiness/Compliance.", language: "id", theme: "default", timer_mode: "global", max_duration_seconds: Some(1200.0), passing_score: None, sections: DISC_SECTIONS, groups: DISC_GROUPS },

    QuizTemplate { id: "cpns_skb", folder: "cpns", subfolder: "SKB", name: "CPNS · SKB", description: "Kompetensi bidang sesuai formasi jabatan.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(3600.0), passing_score: Some(65.0), sections: CPNS_SKB_SECTIONS, groups: CPNS_SKB_GROUPS },
    QuizTemplate { id: "bumn_agility", folder: "bumn", subfolder: "Learning Agility", name: "BUMN · Learning Agility", description: "Situasi perubahan dan pengenalan pola, penilaian berbobot.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(1800.0), passing_score: Some(60.0), sections: BUMN_AGILITY_SECTIONS, groups: BUMN_AGILITY_GROUPS },

    QuizTemplate { id: "osn_matematika", folder: "osn", subfolder: "Matematika", name: "OSN · Matematika", description: "Isian singkat dan uraian pembuktian.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(12600.0), passing_score: Some(50.0), sections: OSN_MIPA_SECTIONS, groups: OSN_MAT_GROUPS },
    QuizTemplate { id: "osn_fisika", folder: "osn", subfolder: "Fisika", name: "OSN · Fisika", description: "Isian bersatuan dan uraian penurunan.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(12600.0), passing_score: Some(50.0), sections: OSN_MIPA_SECTIONS, groups: OSN_FIS_GROUPS },
    QuizTemplate { id: "osn_kimia", folder: "osn", subfolder: "Kimia", name: "OSN · Kimia", description: "Isian dan uraian mekanisme reaksi.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(12600.0), passing_score: Some(50.0), sections: OSN_MIPA_SECTIONS, groups: OSN_KIM_GROUPS },
    QuizTemplate { id: "osn_biologi", folder: "osn", subfolder: "Biologi", name: "OSN · Biologi", description: "Pilihan ganda luas dan analisis data percobaan.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(9000.0), passing_score: Some(50.0), sections: OSN_BIO_SECTIONS, groups: OSN_BIO_GROUPS },
    QuizTemplate { id: "osn_ekonomi", folder: "osn", subfolder: "Ekonomi", name: "OSN · Ekonomi", description: "Teori ekonomi dan studi kasus kebijakan.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(9000.0), passing_score: Some(50.0), sections: OSN_EKO_SECTIONS, groups: OSN_EKO_GROUPS },
    QuizTemplate { id: "osn_geografi", folder: "osn", subfolder: "Geografi", name: "OSN · Geografi", description: "Teori geosfer dan analisis peta/citra.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(9000.0), passing_score: Some(50.0), sections: OSN_GEO_SECTIONS, groups: OSN_GEO_GROUPS },

    QuizTemplate { id: "hsk_2", folder: "hsk", subfolder: "HSK 2", name: "HSK 2", description: "听力 35 + 阅读 25 · 60 soal.", language: "zh", theme: "default", timer_mode: "section", max_duration_seconds: Some(2820.0), passing_score: Some(60.0), sections: HSK2_SECTIONS, groups: HSK2_GROUPS },
    QuizTemplate { id: "hsk_3", folder: "hsk", subfolder: "HSK 3", name: "HSK 3", description: "听力 40 + 阅读 30 + 书写 10 · 80 soal.", language: "zh", theme: "default", timer_mode: "section", max_duration_seconds: Some(4800.0), passing_score: Some(60.0), sections: HSK3_SECTIONS, groups: HSK3_GROUPS },
    QuizTemplate { id: "hsk_5", folder: "hsk", subfolder: "HSK 5", name: "HSK 5", description: "听力 45 + 阅读 45 + 书写 10 · 100 soal.", language: "zh", theme: "default", timer_mode: "section", max_duration_seconds: Some(6900.0), passing_score: Some(60.0), sections: HSK5_SECTIONS, groups: HSK5_GROUPS },
    QuizTemplate { id: "hsk_6", folder: "hsk", subfolder: "HSK 6", name: "HSK 6", description: "听力 50 + 阅读 50 + 书写 1 · 101 soal.", language: "zh", theme: "default", timer_mode: "section", max_duration_seconds: Some(7800.0), passing_score: Some(60.0), sections: HSK6_SECTIONS, groups: HSK6_GROUPS },

    QuizTemplate { id: "jlpt_n5", folder: "jlpt", subfolder: "N5", name: "JLPT N5", description: "文字・語彙, 文法・読解, 聴解.", language: "ja", theme: "default", timer_mode: "section", max_duration_seconds: Some(5400.0), passing_score: Some(50.0), sections: JLPT_N5_SECTIONS, groups: JLPT_N5_GROUPS },
    QuizTemplate { id: "jlpt_n4", folder: "jlpt", subfolder: "N4", name: "JLPT N4", description: "文字・語彙, 文法・読解, 聴解.", language: "ja", theme: "default", timer_mode: "section", max_duration_seconds: Some(6900.0), passing_score: Some(50.0), sections: JLPT_N4_SECTIONS, groups: JLPT_N4_GROUPS },
    QuizTemplate { id: "jlpt_n2", folder: "jlpt", subfolder: "N2", name: "JLPT N2", description: "言語知識・読解 (105 menit) + 聴解 (50 menit).", language: "ja", theme: "default", timer_mode: "section", max_duration_seconds: Some(9300.0), passing_score: Some(50.0), sections: JLPT_N2_SECTIONS, groups: JLPT_N2_GROUPS },
    QuizTemplate { id: "jlpt_n1", folder: "jlpt", subfolder: "N1", name: "JLPT N1", description: "言語知識・読解 (110 menit) + 聴解 (60 menit).", language: "ja", theme: "default", timer_mode: "section", max_duration_seconds: Some(10200.0), passing_score: Some(50.0), sections: JLPT_N1_SECTIONS, groups: JLPT_N1_GROUPS },

    QuizTemplate { id: "delf_a1", folder: "delf", subfolder: "A1", name: "DELF A1", description: "CO, CE, PE, PO tingkat A1.", language: "fr", theme: "default", timer_mode: "section", max_duration_seconds: Some(5400.0), passing_score: Some(50.0), sections: DELF_A1_SECTIONS, groups: DELF_A1_GROUPS },
    QuizTemplate { id: "delf_a2", folder: "delf", subfolder: "A2", name: "DELF A2", description: "CO, CE, PE, PO tingkat A2.", language: "fr", theme: "default", timer_mode: "section", max_duration_seconds: Some(6720.0), passing_score: Some(50.0), sections: DELF_A2_SECTIONS, groups: DELF_A2_GROUPS },
    QuizTemplate { id: "delf_b2", folder: "delf", subfolder: "B2", name: "DELF B2", description: "CO, CE, PE, PO tingkat B2.", language: "fr", theme: "default", timer_mode: "section", max_duration_seconds: Some(10200.0), passing_score: Some(50.0), sections: DELF_B2_SECTIONS, groups: DELF_B2_GROUPS },

    QuizTemplate { id: "goethe_a1", folder: "goethe", subfolder: "A1", name: "Goethe-Zertifikat A1", description: "Lesen, Hören, Schreiben, Sprechen tingkat A1.", language: "de", theme: "default", timer_mode: "section", max_duration_seconds: Some(4800.0), passing_score: Some(60.0), sections: GOETHE_A1_SECTIONS, groups: GOETHE_A1_GROUPS },
    QuizTemplate { id: "goethe_a2", folder: "goethe", subfolder: "A2", name: "Goethe-Zertifikat A2", description: "Lesen, Hören, Schreiben, Sprechen tingkat A2.", language: "de", theme: "default", timer_mode: "section", max_duration_seconds: Some(6300.0), passing_score: Some(60.0), sections: GOETHE_A2_SECTIONS, groups: GOETHE_A2_GROUPS },
    QuizTemplate { id: "goethe_b2", folder: "goethe", subfolder: "B2", name: "Goethe-Zertifikat B2", description: "Lesen, Hören, Schreiben, Sprechen tingkat B2.", language: "de", theme: "default", timer_mode: "section", max_duration_seconds: Some(11700.0), passing_score: Some(60.0), sections: GOETHE_B2_SECTIONS, groups: GOETHE_B2_GROUPS },
    QuizTemplate { id: "akm_literasi", folder: "akm", subfolder: "Literasi", name: "AKM · Literasi Membaca", description: "Satu stimulus, enam format soal khas AKM.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(1800.0), passing_score: Some(65.0), sections: AKM_LIT_SECTIONS, groups: AKM_LIT_GROUPS },
    QuizTemplate { id: "akm_numerasi", folder: "akm", subfolder: "Numerasi", name: "AKM · Numerasi", description: "Stimulus data dengan Benar-Salah, isian, dan uraian.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(1800.0), passing_score: Some(65.0), sections: AKM_NUM_SECTIONS, groups: AKM_NUM_GROUPS },

    QuizTemplate { id: "cpns_skd", folder: "cpns", subfolder: "SKD", name: "CPNS · SKD Lengkap", description: "110 soal · 100 menit. TWK 30, TIU 35, TKP 45 berbobot.", language: "id", theme: "default", timer_mode: "global", max_duration_seconds: Some(6000.0), passing_score: Some(65.0), sections: CPNS_SECTIONS, groups: CPNS_GROUPS },
    QuizTemplate { id: "bumn_rekrutmen", folder: "bumn", subfolder: "Tahap 1", name: "BUMN · TKD + AKHLAK", description: "TKD dan Core Values AKHLAK dengan penilaian berbobot.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(3900.0), passing_score: Some(60.0), sections: BUMN_SECTIONS, groups: BUMN_GROUPS },

    QuizTemplate { id: "osn_umum", folder: "osn", subfolder: "Umum", name: "OSN · Isian & Uraian", description: "Format olimpiade sains: isian singkat dan uraian pembuktian.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(12600.0), passing_score: Some(50.0), sections: OSN_SECTIONS, groups: OSN_GROUPS },
    QuizTemplate { id: "osn_informatika", folder: "osn", subfolder: "Informatika", name: "OSN · Informatika", description: "Analitika tanpa koding dan sesi pemrograman.", language: "id", theme: "default", timer_mode: "section", max_duration_seconds: Some(10800.0), passing_score: Some(50.0), sections: OSN_INF_SECTIONS, groups: OSN_INF_GROUPS },

    QuizTemplate { id: "ielts_reading", folder: "ielts", subfolder: "Academic Reading", name: "IELTS · Academic Reading", description: "3 passage · 40 soal · 60 menit, dengan varian drag-and-drop.", language: "en", theme: "ielts", timer_mode: "global", max_duration_seconds: Some(3600.0), passing_score: Some(65.0), sections: IELTS_READING_SECTIONS, groups: IELTS_READING_GROUPS },
    QuizTemplate { id: "ielts_listening", folder: "ielts", subfolder: "Listening", name: "IELTS · Listening", description: "4 section · 40 soal · 30 menit.", language: "en", theme: "ielts", timer_mode: "audio", max_duration_seconds: Some(1800.0), passing_score: Some(65.0), sections: IELTS_LISTENING_SECTIONS, groups: IELTS_LISTENING_GROUPS },
    QuizTemplate { id: "ielts_writing", folder: "ielts", subfolder: "Writing", name: "IELTS · Academic Writing", description: "Task 1 + Task 2 · 60 menit.", language: "en", theme: "ielts", timer_mode: "section", max_duration_seconds: Some(3600.0), passing_score: Some(65.0), sections: IELTS_WRITING_SECTIONS, groups: IELTS_WRITING_GROUPS },
    QuizTemplate { id: "ielts_speaking", folder: "ielts", subfolder: "Speaking", name: "IELTS · Speaking", description: "3 part · ~14 menit.", language: "en", theme: "ielts", timer_mode: "section", max_duration_seconds: Some(840.0), passing_score: Some(65.0), sections: IELTS_SPEAKING_SECTIONS, groups: IELTS_SPEAKING_GROUPS },
    QuizTemplate { id: "ielts_gt_reading", folder: "ielts", subfolder: "General Training Reading", name: "IELTS · General Training Reading", description: "3 section · 40 soal · 60 menit, teks sehari-hari & kerja.", language: "en", theme: "ielts", timer_mode: "global", max_duration_seconds: Some(3600.0), passing_score: Some(65.0), sections: IELTS_GT_READING_SECTIONS, groups: IELTS_GT_READING_GROUPS },
    QuizTemplate { id: "ielts_gt_writing", folder: "ielts", subfolder: "General Training Writing", name: "IELTS · General Training Writing", description: "Task 1 (surat) + Task 2 · 60 menit.", language: "en", theme: "ielts", timer_mode: "section", max_duration_seconds: Some(3600.0), passing_score: Some(65.0), sections: IELTS_GT_WRITING_SECTIONS, groups: IELTS_GT_WRITING_GROUPS },

    QuizTemplate { id: "toefl_ibt", folder: "toefl", subfolder: "iBT", name: "TOEFL iBT · Full Test", description: "Reading, Listening, Speaking, Writing.", language: "en", theme: "default", timer_mode: "section", max_duration_seconds: Some(7020.0), passing_score: Some(60.0), sections: TOEFL_IBT_SECTIONS, groups: TOEFL_IBT_GROUPS },
    QuizTemplate { id: "toefl_itp", folder: "toefl", subfolder: "ITP", name: "TOEFL ITP · Full Test", description: "140 soal · 115 menit.", language: "en", theme: "default", timer_mode: "section", max_duration_seconds: Some(6900.0), passing_score: Some(60.0), sections: TOEFL_ITP_SECTIONS, groups: TOEFL_ITP_GROUPS },
    QuizTemplate { id: "toeic_lr", folder: "toeic", subfolder: "Listening & Reading", name: "TOEIC · Listening & Reading", description: "200 soal · 120 menit.", language: "en", theme: "default", timer_mode: "section", max_duration_seconds: Some(7200.0), passing_score: Some(60.0), sections: TOEIC_SECTIONS, groups: TOEIC_GROUPS },
    QuizTemplate { id: "toeic_sw", folder: "toeic", subfolder: "Speaking & Writing", name: "TOEIC · Speaking & Writing", description: "11 soal speaking + 8 soal writing · 80 menit.", language: "en", theme: "default", timer_mode: "section", max_duration_seconds: Some(4800.0), passing_score: Some(60.0), sections: TOEIC_SW_SECTIONS, groups: TOEIC_SW_GROUPS },
    QuizTemplate { id: "pte_academic", folder: "pte", subfolder: "Academic", name: "PTE Academic · Full Test", description: "Speaking & Writing, Reading, Listening.", language: "en", theme: "pte", timer_mode: "section", max_duration_seconds: Some(6840.0), passing_score: Some(60.0), sections: PTE_SECTIONS, groups: PTE_GROUPS },
    QuizTemplate { id: "pte_core", folder: "pte", subfolder: "Core", name: "PTE Core", description: "Speaking & Writing, Reading, Listening · 65 menit.", language: "en", theme: "pte", timer_mode: "section", max_duration_seconds: Some(3900.0), passing_score: Some(60.0), sections: PTE_CORE_SECTIONS, groups: PTE_CORE_GROUPS },
    QuizTemplate { id: "duolingo_english", folder: "duolingo", subfolder: "Adaptive", name: "Duolingo English Test", description: "Campuran soal adaptif · ~45 menit.", language: "en", theme: "default", timer_mode: "global", max_duration_seconds: Some(2700.0), passing_score: Some(60.0), sections: DET_SECTIONS, groups: DET_GROUPS },

    QuizTemplate { id: "hsk_1", folder: "hsk", subfolder: "HSK 1", name: "HSK 1", description: "听力 20 + 阅读 20 · 40 soal.", language: "zh", theme: "default", timer_mode: "section", max_duration_seconds: Some(1920.0), passing_score: Some(60.0), sections: HSK1_SECTIONS, groups: HSK1_GROUPS },
    QuizTemplate { id: "hsk_4", folder: "hsk", subfolder: "HSK 4", name: "HSK 4", description: "听力 45 + 阅读 40 + 书写 15 · 100 soal.", language: "zh", theme: "default", timer_mode: "section", max_duration_seconds: Some(5700.0), passing_score: Some(60.0), sections: HSK4_SECTIONS, groups: HSK4_GROUPS },

    QuizTemplate { id: "jlpt_n3", folder: "jlpt", subfolder: "N3", name: "JLPT N3", description: "文字・語彙, 文法・読解, 聴解.", language: "ja", theme: "default", timer_mode: "section", max_duration_seconds: Some(6300.0), passing_score: Some(50.0), sections: JLPT_SECTIONS, groups: JLPT_GROUPS },
    QuizTemplate { id: "topik_1", folder: "topik", subfolder: "TOPIK I", name: "TOPIK I", description: "듣기 30 + 읽기 40 · 100 menit.", language: "ko", theme: "default", timer_mode: "section", max_duration_seconds: Some(6000.0), passing_score: Some(50.0), sections: TOPIK1_SECTIONS, groups: TOPIK1_GROUPS },
    QuizTemplate { id: "topik_2", folder: "topik", subfolder: "TOPIK II", name: "TOPIK II", description: "듣기 50 + 쓰기 4 + 읽기 50.", language: "ko", theme: "default", timer_mode: "section", max_duration_seconds: Some(10800.0), passing_score: Some(50.0), sections: TOPIK2_SECTIONS, groups: TOPIK2_GROUPS },
    QuizTemplate { id: "delf_b1", folder: "delf", subfolder: "B1", name: "DELF B1", description: "CO, CE, PE, PO.", language: "fr", theme: "default", timer_mode: "section", max_duration_seconds: Some(6900.0), passing_score: Some(50.0), sections: DELF_SECTIONS, groups: DELF_GROUPS },
    QuizTemplate { id: "goethe_b1", folder: "goethe", subfolder: "B1", name: "Goethe-Zertifikat B1", description: "Lesen, Hören, Schreiben, Sprechen.", language: "de", theme: "default", timer_mode: "section", max_duration_seconds: Some(8100.0), passing_score: Some(60.0), sections: GOETHE_SECTIONS, groups: GOETHE_GROUPS },
];

pub fn find(id: &str) -> Option<&'static QuizTemplate> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// Turn a template into an empty, correctly-shaped `quiz_config`.
/// Question numbers run 1..N across the whole paper, which is what the
/// learner's answer map and `validate_structure` both require.
pub fn apply(template: &QuizTemplate) -> serde_json::Value {
    let sections: Vec<serde_json::Value> = template
        .sections
        .iter()
        .map(|s| {
            serde_json::json!({
                "section_id": s.id,
                "title": s.title,
                "context_prompt": s.context_prompt,
                "duration_seconds": s.duration_seconds,
            })
        })
        .collect();

    let mut next_number = 1i64;
    let groups: Vec<serde_json::Value> = template
        .groups
        .iter()
        .enumerate()
        .map(|(index, g)| {
            let questions: Vec<serde_json::Value> = (0..g.count)
                .map(|_| {
                    let number = next_number;
                    next_number += 1;
                    serde_json::json!({ "number": number })
                })
                .collect();
            let mut group = serde_json::json!({
                "group_id": format!("{}-g{}", template.id, index + 1),
                "type": g.subtype,
                "section_id": g.section_ref,
                "instruction": g.instruction,
                "context_prompt": g.context_prompt,
                "questions": questions,
            });
            if let Some(mode) = g.display_mode {
                group["display_mode"] = serde_json::Value::String(mode.to_string());
            }
            group
        })
        .collect();

    serde_json::json!({
        "title": template.name,
        "language": template.language,
        "theme": template.theme,
        "timer_mode": template.timer_mode,
        "max_duration_seconds": template.max_duration_seconds,
        "passing_score": template.passing_score,
        "sections": sections,
        "question_groups": groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_label_formats_hours_and_minutes() {
        let hours_only = sec("x", "X", "", 60.0);
        let mixed = sec("x", "X", "", 90.0);
        let minutes_only = sec("x", "X", "", 45.0);
        let make = |duration: Option<f64>| QuizTemplate {
            id: "t",
            folder: "f",
            subfolder: "",
            name: "n",
            description: "",
            language: "id",
            theme: "default",
            timer_mode: "global",
            max_duration_seconds: duration,
            passing_score: None,
            sections: &[],
            groups: &[],
        };
        assert_eq!(make(hours_only.duration_seconds).duration_label().as_deref(), Some("1 jam"));
        assert_eq!(make(mixed.duration_seconds).duration_label().as_deref(), Some("1 jam 30 menit"));
        assert_eq!(make(minutes_only.duration_seconds).duration_label().as_deref(), Some("45 menit"));
        assert_eq!(make(None).duration_label(), None);
    }

    #[test]
    fn difficulty_tracks_the_passing_bar() {
        let make = |passing_score: Option<f64>| QuizTemplate {
            id: "t",
            folder: "f",
            subfolder: "",
            name: "n",
            description: "",
            language: "id",
            theme: "default",
            timer_mode: "global",
            max_duration_seconds: None,
            passing_score,
            sections: &[],
            groups: &[],
        };
        assert_eq!(make(Some(75.0)).difficulty(), quiz_subtype::Difficulty::Hard);
        assert_eq!(make(Some(55.0)).difficulty(), quiz_subtype::Difficulty::Medium);
        assert_eq!(make(Some(30.0)).difficulty(), quiz_subtype::Difficulty::Easy);
        assert_eq!(make(None).difficulty(), quiz_subtype::Difficulty::Medium);
    }

    #[test]
    fn tags_are_the_real_families_the_templates_groups_use() {
        // A real template rather than a hand-built one — the point is
        // that this reads its families off the actual subtype registry.
        let template = TEMPLATES.iter().find(|t| t.id == "psikotes_kerja").expect("psikotes_kerja exists");
        let tags = template.tags();
        assert!(tags.contains(&quiz_subtype::SubtypeFamily::Vocabulary)); // analogy
        assert!(tags.contains(&quiz_subtype::SubtypeFamily::Interactive)); // likert_scale
        // No duplicates even though multiple groups share a family
        // (e.g. two "reading"-family groups both contribute Reading once).
        let mut sorted = tags.clone();
        sorted.sort_by_key(|f| format!("{f:?}"));
        sorted.dedup();
        assert_eq!(sorted.len(), tags.len());
    }

    #[test]
    fn every_template_id_is_unique() {
        let mut ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate template id");
    }

    #[test]
    fn every_template_references_real_subtypes_and_variants() {
        // A template naming a subtype that does not exist, or asking for
        // a variant that subtype cannot render, would only be discovered
        // by an author applying it and getting a 422.
        for template in TEMPLATES {
            for group in template.groups {
                let info = crate::services::quiz_subtype::find(group.subtype)
                    .unwrap_or_else(|| panic!("template {} references unknown subtype {}", template.id, group.subtype));
                if let Some(mode) = group.display_mode {
                    let ok = info.variants.iter().any(|v| match v {
                        crate::services::quiz_subtype::DisplayVariant::Default => mode == "default",
                        crate::services::quiz_subtype::DisplayVariant::Ielts => mode == "ielts",
                        crate::services::quiz_subtype::DisplayVariant::Grid => mode == "grid",
                    });
                    assert!(ok, "template {} asks {} for display_mode {mode}, which it does not support", template.id, group.subtype);
                }
                assert!(
                    template.sections.iter().any(|s| s.id == group.section_ref),
                    "template {} has a group pointing at missing section {}",
                    template.id,
                    group.section_ref
                );
            }
        }
    }

    // Regression — cpns_skb authored one group with count=80. Every
    // group's questions are filled by a single "Generate soal" call
    // (generate_quiz_group), which refuses count > 50 outright — a
    // group over that cap can never actually be AI-generated at all, a
    // gap the QA sweep of every template (Phase 38) only caught by
    // trying to apply and generate every one for real.
    #[test]
    fn no_group_asks_for_more_than_one_generate_call_can_produce() {
        const MAX_GENERATABLE_COUNT: i64 = 50;
        for template in TEMPLATES {
            for group in template.groups {
                assert!(
                    group.count <= MAX_GENERATABLE_COUNT,
                    "template {} group ({} x{}) exceeds generate_quiz_group's {MAX_GENERATABLE_COUNT}-question cap — split it across more groups",
                    template.id,
                    group.subtype,
                    group.count,
                );
            }
        }
    }

    #[test]
    fn every_template_folder_exists() {
        for template in TEMPLATES {
            assert!(FOLDERS.iter().any(|f| f.id == template.folder), "template {} is in unknown folder {}", template.id, template.folder);
        }
    }

    #[test]
    fn applying_a_template_produces_a_valid_config() {
        // The whole point: a template must apply cleanly, or the author
        // gets a broken paper. This runs the real validator over all of
        // them, including the unique-question-number rule.
        for template in TEMPLATES {
            let config = apply(template);
            crate::services::quiz_config_schema::validate_structure(&config)
                .unwrap_or_else(|e| panic!("template {} produced an invalid config: {e:?}", template.id));
        }
    }

    /// Every `module_labels.kind = 'ujian'` value in the learning paths,
    /// mapped to the template that covers it. Kept here rather than
    /// derived at runtime because the point is to FAIL when someone adds
    /// an exam to a learning path without a paper to sit it on.
    const EXAM_COVERAGE: &[(&str, &str)] = &[
        ("utbk-pu", "snbt_pu"),
        ("utbk-ppu", "snbt_ppu"),
        ("utbk-pbm", "snbt_pbm"),
        ("utbk-pk", "snbt_pk"),
        ("utbk-lbind", "snbt_literasi_id"),
        ("utbk-lbing", "snbt_literasi_en"),
        ("utbk-pm", "snbt_pm"),
        ("utbk-saintek", "tka_saintek"),
        ("utbk-soshum", "tka_soshum"),
        ("tka-saintek", "tka_saintek"),
        ("tka-soshum", "tka_soshum"),
        ("tka-literasi", "tka_literasi"),
        ("tka-numerasi", "tka_numerasi"),
        ("tka-inggris", "tka_inggris"),
        ("mandiri-ptn", "mandiri_ptn"),
        ("um-ptkin", "um_ptkin"),
        ("akm-literasi", "akm_literasi"),
        ("akm-numerasi", "akm_numerasi"),
        ("cpns-twk", "cpns_skd"),
        ("cpns-tiu", "cpns_skd"),
        ("cpns-tkp", "cpns_skd"),
        ("cpns-skb", "cpns_skb"),
        ("bumn-tkd", "bumn_rekrutmen"),
        ("bumn-akhlak", "bumn_rekrutmen"),
        ("bumn-agility", "bumn_agility"),
        ("osn-matematika", "osn_matematika"),
        ("osn-fisika", "osn_fisika"),
        ("osn-kimia", "osn_kimia"),
        ("osn-biologi", "osn_biologi"),
        ("osn-ekonomi", "osn_ekonomi"),
        ("osn-geografi", "osn_geografi"),
        ("osn-informatika", "osn_informatika"),
        ("ielts", "ielts_reading"),
        ("toefl-ibt", "toefl_ibt"),
        ("toefl-itp", "toefl_itp"),
        ("toeic", "toeic_lr"),
        ("pte-academic", "pte_academic"),
        ("duolingo-english", "duolingo_english"),
        ("hsk-1", "hsk_1"),
        ("hsk-2", "hsk_2"),
        ("hsk-3", "hsk_3"),
        ("hsk-4", "hsk_4"),
        ("hsk-5", "hsk_5"),
        ("jlpt-n5", "jlpt_n5"),
        ("jlpt-n4", "jlpt_n4"),
        ("jlpt-n3", "jlpt_n3"),
        ("jlpt-n2", "jlpt_n2"),
        // TOPIK levels 1-4 are OUTCOMES of two papers, not four papers:
        // TOPIK I awards level 1-2, TOPIK II awards level 3-6.
        ("topik-1", "topik_1"),
        ("topik-2", "topik_1"),
        ("topik-3", "topik_2"),
        ("topik-4", "topik_2"),
        ("delf-a1", "delf_a1"),
        ("delf-a2", "delf_a2"),
        ("delf-b1", "delf_b1"),
        ("delf-b2", "delf_b2"),
        ("goethe-a1", "goethe_a1"),
        ("goethe-a2", "goethe_a2"),
        ("goethe-b1", "goethe_b1"),
        ("goethe-b2", "goethe_b2"),
    ];

    #[test]
    fn every_exam_in_the_learning_paths_has_a_template() {
        for (exam, template_id) in EXAM_COVERAGE {
            assert!(find(template_id).is_some(), "exam label {exam} maps to missing template {template_id}");
        }
    }

    #[test]
    fn question_numbers_run_continuously_across_the_paper() {
        let template = find("snbt_pu").unwrap();
        let config = apply(template);
        let numbers: Vec<i64> = config["question_groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["questions"].as_array().unwrap().iter().map(|q| q["number"].as_i64().unwrap()))
            .collect();
        assert_eq!(numbers, (1..=template.question_count()).collect::<Vec<_>>());
    }
}
