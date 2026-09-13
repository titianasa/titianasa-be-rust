// Phase 37 — the output-shape catalogue the quiz generator prompts with.
//
// ~40 subtypes collapse to 15 SHAPES: the JSON the model must emit, and
// the rules it must follow to emit it. That collapse is the whole reason
// adding a subtype rarely costs anything — `analogy`, `image_word` and
// `stress_pattern` are all `Mcq`, so they share one schema, one parser
// and one set of rules, and differ only in the per-subtype hint appended
// to the prompt.
//
// Three things drive the prompt, in order of specificity:
//   1. the SHAPE decides the JSON schema and its hard rules,
//   2. the SUBTYPE adds a hint about what kind of question to write,
//   3. the author's `context_prompt` (from the template or typed by
//      hand) says what this particular group is about.

use crate::services::quiz_subtype::SubtypeShape;

pub struct ShapeSpec {
    pub description: &'static str,
    /// A filled-in example, not an abstract placeholder — a real one
    /// stops the model nesting fields in the wrong place, which an
    /// abstract shape reliably fails to.
    pub schema: &'static str,
    pub rules: &'static [&'static str],
    /// Top-level resources the model may return alongside `questions`,
    /// which the caller merges into the group.
    pub emits: &'static [&'static str],
}

/// A handful of shapes hard-cap how many questions one group can ever
/// hold — `HighlightWords` scores its whole transcript as ONE question
/// (see its rules below), so asking the model for "3 soal" on top of
/// that contradicts the shape's own spec and reliably produced zero
/// questions rather than either number. `build_prompt` uses this to
/// override the caller's requested count instead of passing it through.
pub fn fixed_question_count(shape: SubtypeShape) -> Option<i64> {
    match shape {
        SubtypeShape::HighlightWords => Some(1),
        _ => None,
    }
}

pub fn spec(shape: SubtypeShape) -> ShapeSpec {
    match shape {
        SubtypeShape::Mcq => ShapeSpec {
            description: "Pilihan ganda satu jawaban benar.",
            schema: r#"{
  "questions": [
    {
      "number": 1,
      "stem": "Ibu kota Indonesia adalah ...",
      "choices": [
        {"label": "A", "text": "Bandung"},
        {"label": "B", "text": "Jakarta"},
        {"label": "C", "text": "Surabaya"},
        {"label": "D", "text": "Medan"}
      ],
      "answer": "B",
      "explanation": "Jakarta adalah ibu kota Indonesia sejak 1945.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}
    }
  ]
}"#,
            rules: &[
                "Tepat 4 pilihan berlabel A/B/C/D, kecuali diminta lain oleh konteks.",
                "Tepat satu jawaban benar; tulis HURUF-nya pada `answer`.",
                "Pengecoh harus masuk akal — kesalahan yang benar-benar sering dilakukan siswa, bukan pilihan yang jelas ngawur.",
                "`explanation` WAJIB ada: jelaskan mengapa kunci benar DAN mengapa pengecoh yang paling menggoda itu salah.",
            ],
            emits: &["passage"],
        },
        SubtypeShape::McqMulti => ShapeSpec {
            description: "Pilihan ganda kompleks — lebih dari satu jawaban benar.",
            schema: r#"{
  "questions": [
    {
      "number": "5-6",
      "stem": "Pilih DUA pernyataan yang benar tentang fotosintesis.",
      "choices": [
        {"label": "A", "text": "Menghasilkan oksigen"},
        {"label": "B", "text": "Terjadi di mitokondria"},
        {"label": "C", "text": "Membutuhkan cahaya"},
        {"label": "D", "text": "Menghasilkan karbon dioksida"},
        {"label": "E", "text": "Hanya terjadi malam hari"}
      ],
      "answer": ["A", "C"],
      "explanation": "Fotosintesis membutuhkan cahaya dan menghasilkan oksigen.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}
    }
  ]
}"#,
            rules: &[
                "`number` berupa rentang (\"5-6\" untuk pilih dua, \"5-7\" untuk pilih tiga) — jumlah slot menentukan poinnya.",
                "`stem` WAJIB menyebut eksplisit berapa jawaban yang harus dipilih.",
                "Sediakan 5 pilihan atau lebih; `answer` berupa array huruf yang diurutkan.",
            ],
            emits: &["passage"],
        },
        SubtypeShape::Tf => ShapeSpec {
            description: "Pernyataan benar/salah atas satu stimulus.",
            schema: r#"{
  "options": ["Benar", "Salah"],
  "questions": [
    {"number": 1, "text": "Produksi padi 2023 melebihi 31 juta ton.", "answer": "Benar", "explanation": "Teks menyebut 31,5 juta ton.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}},
    {"number": 2, "text": "Produksi padi turun dibanding tahun sebelumnya.", "answer": "Salah", "explanation": "Teks menyebut kenaikan dari 30 juta ton.", "taxonomy": {"difficulty": "sulit", "bloom": "c4"}}
  ]
}"#,
            rules: &[
                "Gunakan `text` untuk pernyataan, BUKAN `stem`.",
                "`options` menentukan label yang dilihat siswa dan HARUS sama persis dengan nilai `answer`. Pakai [\"Benar\",\"Salah\"] untuk soal berbahasa Indonesia, [\"True\",\"False\",\"Not Given\"] untuk IELTS, [\"对\",\"错\"] untuk HSK, [\"Vrai\",\"Faux\"] untuk DELF, [\"Richtig\",\"Falsch\"] untuk Goethe.",
                "Jika tersedia opsi \"Not Given\", pakai HANYA bila pernyataan benar-benar tidak disinggung stimulus — bukan sekadar inferensi yang masuk akal.",
                "Seimbangkan jumlah jawaban antar pilihan.",
            ],
            emits: &["passage", "options"],
        },
        SubtypeShape::SingleText => ShapeSpec {
            description: "Jawaban teks singkat, dicocokkan toleran.",
            schema: r#"{
  "questions": [
    {"number": 1, "stem": "Berapa lama proses pemulihan berlangsung?", "answer": "10 tahun|sepuluh tahun", "explanation": "Teks menyebut pemulihan memakan waktu 10 tahun.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "Jawaban maksimal 3 kata dan HARUS muncul apa adanya di stimulus (kecuali soal hitungan).",
                "Pisahkan jawaban alternatif yang sama-sama benar dengan tanda `|`.",
                "Untuk jawaban berupa angka, tulis angkanya saja tanpa satuan; satuan diletakkan di pertanyaan.",
            ],
            emits: &["passage"],
        },
        SubtypeShape::Matching => ShapeSpec {
            description: "Mencocokkan label kiri dengan opsi dari satu pool bersama.",
            schema: r#"{
  "options": [
    {"label": "A", "text": "fotosintesis"},
    {"label": "B", "text": "respirasi"}
  ],
  "questions": [
    {"number": 1, "label": "Menghasilkan oksigen", "answer": "fotosintesis", "explanation": "Fotosintesis melepaskan oksigen.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "`options` adalah pool bersama untuk SEMUA soal di grup ini.",
                "Gunakan `label` untuk sisi kiri yang dicocokkan, BUKAN `stem`.",
                "`answer` ditulis sebagai TEKS opsi (bukan hurufnya).",
                "Sediakan 1-2 opsi pengecoh yang tidak terpakai, kecuali konteks meminta setiap opsi terpakai.",
                "Bila opsi berupa gambar, tambahkan `image` berisi URL pada tiap opsi.",
            ],
            emits: &["passage", "options"],
        },
        SubtypeShape::MatchingHeadings => ShapeSpec {
            description: "Mencocokkan tiap bagian bacaan dengan judul yang paling tepat.",
            schema: r#"{
  "headings": [
    {"label": "i", "text": "Penyebab menurunnya populasi"},
    {"label": "ii", "text": "Upaya pemulihan"}
  ],
  "passage_sections": [
    {"label": "A", "question_number": 1, "text": "Paragraf pertama ..."},
    {"label": "B", "question_number": 2, "text": "Paragraf kedua ..."}
  ],
  "questions": [
    {"number": 1, "answer": "Penyebab menurunnya populasi", "explanation": "Bagian A membahas sebab menurunnya populasi.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "Gunakan `headings` (BUKAN `options`) untuk pool judul; beri label angka romawi kecil.",
                "Tiap judul menangkap GAGASAN UTAMA satu bagian, bukan detailnya.",
                "Sertakan 2-3 judul pengecoh.",
                "`passage_sections` memecah bacaan per bagian berlabel; tiap bagian yang ditanyakan punya `question_number`.",
            ],
            emits: &["passage", "headings", "passage_sections"],
        },
        SubtypeShape::GapFillRich => ShapeSpec {
            description: "Melengkapi rumpang — bisa tata letak catatan berumpang inline, atau daftar kalimat rumpang.",
            schema: r#"{
  "sections": [
    {
      "label": "Catatan",
      "rows": [
        {"category": "Penyebab", "items": [{"text": "Suhu laut naik hingga"}, {"question_number": 1}, {"text": "derajat."}]},
        {"category": "Dampak", "items": [{"text": "Terumbu memutih pada tahun"}, {"question_number": 2}]}
      ]
    }
  ],
  "questions": [
    {"number": 1, "answer": "2", "explanation": "Teks menyebut kenaikan 2 derajat.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}},
    {"number": 2, "answer": "2016", "explanation": "Peristiwa pemutihan terjadi pada 2016.", "taxonomy": {"difficulty": "sulit", "bloom": "c4"}}
  ]
}"#,
            rules: &[
                "Pilih SALAH SATU tata letak: `sections` berumpang inline (gaya note completion), ATAU tiap soal punya `stem` sendiri yang memuat `___`.",
                "Setiap `question_number` di dalam `sections` WAJIB punya entri di `questions`.",
                "Jawaban maksimal 3 kata dan muncul apa adanya di stimulus.",
                "Jika grup memakai word bank (drag-and-drop), isi juga `options` dengan pool kata beserta pengecoh.",
            ],
            emits: &["passage", "sections", "options"],
        },
        SubtypeShape::TableFill => ShapeSpec {
            description: "Melengkapi sel tabel.",
            schema: r#"{
  "columns": ["Aspek", "Nilai"],
  "rows": [
    {"cells": [{"text": "Luas tutupan"}, {"question_number": 1}]},
    {"cells": [{"text": "Tahun keruntuhan"}, {"question_number": 2}]}
  ],
  "questions": [
    {"number": 1, "answer": "1%", "explanation": "Teks menyebut kurang dari 1% dasar laut.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "`columns` adalah judul kolom; setiap baris punya jumlah sel PERSIS sama dengan jumlah kolom.",
                "Sel statis memakai `{\"text\": \"...\"}`; sel rumpang memakai `{\"question_number\": N}`.",
                "Untuk sel yang memuat teks dan rumpang sekaligus, pakai `{\"lines\": [[{\"text\":\"...\"},{\"question_number\":N}]]}`.",
                "Setiap `question_number` WAJIB punya entri di `questions`.",
            ],
            emits: &["passage", "columns", "rows"],
        },
        SubtypeShape::FlowFill => ShapeSpec {
            description: "Melengkapi langkah pada diagram alur.",
            schema: r#"{
  "options": ["nektar", "sarang"],
  "flow": [
    {"text": "Lebah mencari"},
    {"question_number": 1},
    {"text": "lalu kembali ke"},
    {"question_number": 2}
  ],
  "questions": [
    {"number": 1, "answer": "nektar", "explanation": "Lebah mengumpulkan nektar dari bunga.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "`flow` adalah urutan linear; langkah statis punya `text`, langkah rumpang punya `question_number`.",
                "Sertakan `options` sebagai pool jawaban beserta pengecoh.",
                "Urutan langkah harus mencerminkan urutan proses pada stimulus.",
            ],
            emits: &["passage", "flow", "options"],
        },
        SubtypeShape::Sequence => ShapeSpec {
            description: "Menyusun potongan menjadi kalimat atau urutan yang benar.",
            schema: r#"{
  "questions": [
    {
      "number": 1,
      "stem": "Susun menjadi kalimat yang benar.",
      "scrambled": ["ke", "sekolah", "Saya", "pergi"],
      "answer": "Saya pergi ke sekolah",
      "explanation": "Pola dasar: Subjek + Predikat + Keterangan.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}
    }
  ]
}"#,
            rules: &[
                "`scrambled` berisi potongan dalam urutan ACAK; `answer` adalah susunan yang benar.",
                "Untuk menyusun huruf menjadi kata, tiap elemen `scrambled` adalah satu huruf.",
                "Potongan pada `scrambled` harus persis sama dengan yang menyusun `answer`.",
            ],
            emits: &[],
        },
        SubtypeShape::Likert => ShapeSpec {
            description: "Inventori skala 1-5; tidak ada jawaban benar atau salah.",
            schema: r#"{
  "questions": [
    {"number": 1, "stem": "Saya nyaman bekerja dalam tim.", "explanation": "Mengukur orientasi kolaboratif.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "JANGAN tulis `answer` — pernyataan ini tidak dinilai benar/salah.",
                "Tulis pernyataan sebagai kalimat orang pertama tentang perilaku, bukan pertanyaan.",
                "Hindari pernyataan bermakna ganda yang mengukur dua hal sekaligus.",
            ],
            emits: &[],
        },
        SubtypeShape::FreeformEval => ShapeSpec {
            description: "Tugas terbuka yang dinilai rubrik oleh AI atau guru.",
            schema: r#"{
  "questions": [
    {
      "number": 1,
      "prompt": "Jelaskan dampak perubahan iklim terhadap pertanian di Indonesia.",
      "min_words": 150,
      "max_words": 300,
      "rubric": "Isi (40), Struktur (30), Bahasa (30)",
      "explanation": "Jawaban kuat menyebut pergeseran musim tanam dan gagal panen.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}
    }
  ]
}"#,
            rules: &[
                "Gunakan `prompt` untuk instruksi tugas, BUKAN `stem`.",
                "JANGAN tulis `answer` — tugas ini dinilai rubrik.",
                "Sertakan `min_words`/`max_words` untuk tugas menulis, atau `max_duration_seconds` untuk tugas merekam.",
                "`explanation` berisi ciri jawaban yang kuat, sebagai panduan penilai.",
            ],
            emits: &[],
        },
        SubtypeShape::InteractiveEmbed => ShapeSpec {
            description: "Sematan interaktif — tidak dibuat oleh AI.",
            schema: r#"{"questions": []}"#,
            rules: &["Tipe ini disiapkan manual oleh penulis; kembalikan `questions` kosong."],
            emits: &[],
        },
        SubtypeShape::HighlightWords => ShapeSpec {
            description: "Menandai kata pada transkrip yang berbeda dari audio.",
            schema: r#"{
  "transcript_words": ["Lebah", "menyerbuki", "sembilan", "puluh", "persen", "tanaman"],
  "questions": [
    {"number": 1, "answer": ["2", "3"], "explanation": "Audio menyebut tujuh puluh persen, bukan sembilan puluh.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "`transcript_words` adalah transkrip yang DITAMPILKAN, sudah dipecah per kata.",
                "Beberapa kata sengaja berbeda dari yang diucapkan audio.",
                "`answer` berisi INDEKS (mulai dari 0) kata-kata yang berbeda, sebagai array string.",
                "Satu grup hanya berisi SATU soal — seluruh transkrip dinilai sebagai satu himpunan jawaban.",
            ],
            emits: &["transcript_words", "transcript"],
        },
        SubtypeShape::SpeakingChallenge => ShapeSpec {
            description: "Aktivitas berbicara berpasangan di kelas, dinilai tutor.",
            schema: r#"{
  "conversation_cards": [
    {"title": "Rencana liburan", "starter": "Ke mana kamu ingin berlibur?", "follow_up_questions": ["Kenapa?", "Dengan siapa?"], "vocab": ["itinerary", "budget"]}
  ],
  "questions": [
    {"number": 1, "prompt": "Lakukan percakapan berpasangan menggunakan kartu di atas.", "explanation": "Tutor menilai kelancaran dan ketepatan kosakata.", "taxonomy": {"difficulty": "sedang", "bloom": "c2"}}
  ]
}"#,
            rules: &[
                "Buat kartu percakapan berisi pemantik dan pertanyaan lanjutan.",
                "JANGAN tulis `answer` — aktivitas ini dinilai tutor secara langsung.",
            ],
            emits: &["conversation_cards"],
        },
    }
}

/// What to write, per subtype — layered on top of its shape's schema.
/// Only subtypes whose content differs meaningfully from their shape's
/// default need an entry here.
pub fn subtype_hint(subtype: &str) -> Option<&'static str> {
    Some(match subtype {
        "analogy" => "Format stem: \"A : B :: C : ?\". Relasi bisa bagian-keseluruhan, fungsi, sebab-akibat, profesi-alat, atau sinonim-antonim. Pengecoh memakai relasi yang mirip tapi tidak tepat.",
        "error_identification" => "`stem` berupa kalimat penuh dengan empat penanda inline \"(A) ... (B) ... (C) ... (D) ...\". `choices` mengutip ulang tiap bagian apa adanya. `answer` adalah huruf bagian yang MENGANDUNG kesalahan tata bahasa; tiga bagian lain harus benar.",
        "cloze_passage" => "Tulis satu paragraf bersama pada `stem` di level ROOT dengan penanda `#1`, `#2`, ... untuk tiap rumpang. Lalu tiap soal berisi `choices` dan `answer` untuk rumpang bernomor itu. JANGAN mengulang paragraf di tiap soal.",
        "error_correction" => "Tiap soal: `sentence` (kalimat lengkap yang memuat kesalahan), `error_text` (kata/frasa salah yang muncul APA ADANYA di dalam `sentence`), dan `answer` (koreksinya). JANGAN pakai `stem`.",
        "spelling" => "Tiap soal: `meaning` (definisi), `phonetic` (pelafalan), dan `answer` (ejaan yang benar). JANGAN pakai `stem`.",
        "word_form" => "Tiap soal: `sentence` dengan `___`, `root_word` dalam HURUF KAPITAL, dan `answer` berupa bentuk kata yang tepat.",
        "vocab_cloze" => "Tiap soal: `sentence` dengan `___`, `hint` (definisi singkat kata target), dan `answer`.",
        "word_match" => "Tiap soal: `word` (kata target), `definition` (artinya), dan `answer` yang sama dengan `definition`.",
        "map_labeling" => "Sertakan `map_description` yang menjelaskan diagram. Tiap `label` soal adalah zona yang harus diberi nama; `answer` adalah teks opsi apa adanya.",
        "minimal_pairs" => "Stem adalah instruksi mendengarkan. Dua pilihan berupa kata yang nyaris sama (mis. ship/sheep).",
        "stress_pattern" => "Stem adalah kata target; pilihan A/B/C/D mewakili suku kata ke-1/2/3/4. `answer` adalah huruf suku kata yang ditekan.",
        "paragraph_editing" => "Tulis paragraf bersama pada `paragraph` di level ROOT. Tiap soal: `error_text` (kata salah, muncul apa adanya di paragraf) dan `answer` (koreksinya).",
        "sentence_transform" => "Stem berisi kalimat awal dan kata kunci wajib. `model_answer` berisi contoh penulisan ulang yang benar.",
        "flashcard" => "Sisi depan pada `stem`, sisi belakang pada `label`. Tidak dinilai.",
        "image_word" => "Sertakan `image_url` pada tiap soal, atau `image` pada tiap pilihan bila yang dipilih adalah gambarnya.",
        "listen_repeat" | "voice_record" => "Stem berisi kalimat atau tugas yang harus diucapkan. Sertakan `max_duration_seconds`.",
        "essay" => "Prompt harus spesifik dan bisa diperdebatkan, bukan pertanyaan pengetahuan umum. Sertakan `min_words` dan rubrik berbobot.",
        "file_upload" => "Sertakan `max_size_mb` dan `allowed_ext` (mis. \".pdf,.docx\").",
        _ => return None,
    })
}

/// Extra instructions triggered by how the group is presented or scored,
/// rather than by its subtype.
pub fn variant_rules(display_mode: Option<&str>, weighted: bool) -> Vec<&'static str> {
    let mut rules = Vec::new();
    if display_mode == Some("ielts") {
        rules.push("Grup ini memakai word bank drag-and-drop: WAJIB isi `options` dengan pool jawaban, termasuk 1-2 pengecoh yang tidak terpakai.");
    }
    if display_mode == Some("grid") {
        rules.push("Grup ini ditampilkan sebagai tabel pernyataan: semua soal memakai pilihan yang sama dari `options`, dan tiap soal adalah satu baris.");
    }
    if weighted {
        rules.push(
            "Soal ini DINILAI BERBOBOT: tulis `option_scores` berupa objek {\"A\": 5, \"B\": 3, \"C\": 2, \"D\": 1} yang memberi nilai 1-5 pada SETIAP pilihan. JANGAN tulis `answer` — tidak ada pilihan yang salah, hanya lebih atau kurang tepat. Nilai 5 untuk tindakan paling ideal, 1 untuk yang paling tidak tepat.",
        );
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::quiz_subtype;

    #[test]
    fn every_shape_has_a_schema_and_rules() {
        for info in quiz_subtype::SUBTYPES {
            let s = spec(info.shape);
            assert!(!s.schema.trim().is_empty(), "{} has an empty schema", info.id);
            assert!(!s.description.is_empty(), "{} has no description", info.id);
        }
    }

    #[test]
    fn every_shape_schema_is_valid_json() {
        // The schema is shown to the model as the thing to imitate; if it
        // is not itself valid JSON, the model reliably copies the flaw.
        for info in quiz_subtype::SUBTYPES {
            let s = spec(info.shape);
            serde_json::from_str::<serde_json::Value>(s.schema)
                .unwrap_or_else(|e| panic!("{} ({:?}) has a malformed schema example: {e}", info.id, info.shape));
        }
    }

    #[test]
    fn weighted_rule_forbids_an_answer_key() {
        let rules = variant_rules(None, true);
        assert!(rules.iter().any(|r| r.contains("option_scores")));
        assert!(rules.iter().any(|r| r.contains("JANGAN tulis `answer`")));
    }

    #[test]
    fn drag_and_drop_demands_an_option_pool() {
        assert!(variant_rules(Some("ielts"), false).iter().any(|r| r.contains("options")));
        assert!(variant_rules(Some("default"), false).is_empty());
    }
}

#[cfg(test)]
mod taxonomy_schema_tests {
    use super::*;

    /// The schema string is what the model copies, so a taxonomy rule in
    /// the prompt that no example demonstrates gets ignored roughly as
    /// often as it gets followed. Every worked example that shows an
    /// `explanation` must show a `taxonomy` beside it — and the example
    /// has to stay parseable, since a malformed one teaches malformed
    /// output.
    #[test]
    fn every_worked_example_is_valid_json_and_classifies_its_questions() {
        use crate::services::quiz_taxonomy::{BloomLevel, DifficultyLevel};

        // Driven off the subtype registry rather than a hand-kept
        // shape list, so a shape added with a new subtype is covered
        // the moment it exists.
        for info in crate::services::quiz_subtype::SUBTYPES {
            let shape = info.shape;
            let spec = spec(shape);
            let parsed: serde_json::Value = serde_json::from_str(spec.schema)
                .unwrap_or_else(|e| panic!("{shape:?} schema is not valid JSON: {e}"));

            let Some(questions) = parsed.get("questions").and_then(|q| q.as_array()) else { continue };
            for q in questions {
                // Only examples that bother to show an explanation are
                // full worked questions; the sparser ones illustrate a
                // structure, not a complete item.
                if q.get("explanation").is_none() {
                    continue;
                }
                let tax = q.get("taxonomy").unwrap_or_else(|| panic!("{shape:?}: a worked example has no `taxonomy`"));
                let difficulty = tax.get("difficulty").and_then(|v| v.as_str()).unwrap_or("");
                let bloom = tax.get("bloom").and_then(|v| v.as_str()).unwrap_or("");
                assert!(DifficultyLevel::parse(difficulty).is_some(), "{shape:?}: bad difficulty {difficulty:?}");
                assert!(BloomLevel::parse(bloom).is_some(), "{shape:?}: bad bloom {bloom:?}");
            }
        }
    }
}
