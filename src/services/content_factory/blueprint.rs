// Deterministic question plans for the `bab_content` kind — a Rust port
// of `agent/tools/content-gen/blueprint.py`, the script that built the
// one bab (Matematika Tahap 1 › "Nilai Tempat: Satuan sampai Jutaan")
// this platform has shipped so far.
//
// A bab's question bank is `BANK_SIZE` questions that must, together,
// hit a target mix of subtype, Bloom level, difficulty and source
// section. Asking the model for that mix per call doesn't work once the
// bank is filled in chunks — ten chunks of five each round their own
// quota and drift from one plan for fifty. So the plan is made here,
// once, as a list of SLOTS; each generate call is told exactly which
// slots to fill, and the filler only asks for what the database doesn't
// have yet (see `deficit`) — which is also what makes a resumed task
// free (nothing already stored is re-requested) and a failed chunk cost
// only itself.
//
// Reuses `quiz_taxonomy::target_spread` for the Bloom/difficulty split
// instead of a second copy of its weight tables — `blueprint.py` had to
// duplicate them because it lived outside the Rust crate; this doesn't.

use crate::services::quiz_taxonomy::{apportion, target_spread};

pub const BANK_SIZE: i64 = 50;
/// Draws the two shorter Latihan make from the bank on every attempt.
pub const DRAW_COUNTS: [i64; 2] = [10, 25];

/// One subtype (one AI call per section): a 4-question comprehension
/// check doesn't need the variety a 50-question bank does, and five
/// extra calls per bab would cost more than that variety is worth.
pub const CHECKPOINT_SUBTYPE: &str = "multiple_choice";
pub const CHECKPOINT_POOL: i64 = 4;
/// Fixed, not jenjang-weighted like the bank: a checkpoint is "did you
/// follow THIS section", not a paper — it stays easy-to-medium at every
/// jenjang on purpose, never reaching for a pascasarjana student's HOTS.
const CHECKPOINT_BLOOM: [&str; 4] = ["c1", "c2", "c2", "c3"];
const CHECKPOINT_DIFFICULTY: [&str; 4] = ["mudah", "mudah", "sedang", "sedang"];

/// What a bank is made of, per the bab plan's `jenis_soal`. `short_answer`
/// matters where there's something to work out — pure multiple choice
/// lets a learner pass by elimination without doing the arithmetic.
/// Where the answer is a concept, an exact-match short answer marks
/// right answers wrong ("fotosintesis" vs "proses fotosintesis"), so
/// multi-select carries the harder recall instead.
pub fn bank_mixes(jenis_soal: &str) -> Vec<(&'static str, u32)> {
    match jenis_soal {
        "konsep" => vec![("multiple_choice", 55), ("true_false", 20), ("multiple_choice_multiple", 25)],
        _ => vec![("multiple_choice", 50), ("true_false", 20), ("short_answer", 30)],
    }
}

pub fn subtype_group(subtype: &str) -> &'static str {
    match subtype {
        "true_false" => "tf",
        "short_answer" => "sa",
        "multiple_choice_multiple" => "mm",
        _ => "mc",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub group_id: String,
    pub subtype: String,
    pub bloom: String,
    pub difficulty: String,
    pub section_id: Option<String>,
}

/// What's actually stored, read fresh before every chunk: (group_id,
/// bloom, difficulty, section_id) per question.
pub type Stored = (String, Option<String>, Option<String>, Option<String>);

fn flatten(counts: Vec<(&'static str, i64)>) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (v, n) in counts {
        for _ in 0..n {
            out.push(v);
        }
    }
    out
}

/// `size` slots: subtype × Bloom × difficulty × section, balanced on
/// every axis. Deterministic — the same bab always gets the same plan,
/// which is what lets a resumed task compare against what's stored.
pub fn bank_blueprint(section_ids: &[String], level: &str, jenis_soal: &str) -> Vec<Slot> {
    let mixes = bank_mixes(jenis_soal);
    let weights: Vec<u32> = mixes.iter().map(|(_, w)| *w).collect();
    let per_subtype = apportion(&weights, BANK_SIZE);
    let mut slots = Vec::new();
    for ((subtype, _), count) in mixes.iter().zip(per_subtype) {
        let (_, bloom_counts, diff_counts) = target_spread(level, count);
        let blooms = flatten(bloom_counts.into_iter().map(|(b, n)| (b.id(), n)).collect());
        let difficulties = flatten(diff_counts.into_iter().map(|(d, n)| (d.id(), n)).collect());
        for i in 0..count as usize {
            slots.push(Slot {
                group_id: subtype_group(subtype).to_string(),
                subtype: (*subtype).to_string(),
                bloom: blooms.get(i).copied().unwrap_or("c2").to_string(),
                difficulty: difficulties.get(i).copied().unwrap_or("sedang").to_string(),
                // Round-robin so every section is tested by every
                // subtype, instead of section 1 taking all the MCs.
                section_id: if section_ids.is_empty() { None } else { Some(section_ids[i % section_ids.len()].clone()) },
            });
        }
    }
    slots
}

pub fn checkpoint_blueprint(section_id: &str) -> Vec<Slot> {
    (0..CHECKPOINT_POOL as usize)
        .map(|i| Slot {
            group_id: "cp".to_string(),
            subtype: CHECKPOINT_SUBTYPE.to_string(),
            bloom: CHECKPOINT_BLOOM[i].to_string(),
            difficulty: CHECKPOINT_DIFFICULTY[i].to_string(),
            section_id: Some(section_id.to_string()),
        })
        .collect()
}

/// Which planned slots aren't in the database yet. A question the model
/// labelled c3 when c4 was asked for still counts as the c3 slot it
/// ended up being — the next chunk asks for what's still missing rather
/// than regenerating the one that drifted. A question that matches no
/// planned slot (a drifted label) still counts against the bank's size,
/// or the bank would grow past `BANK_SIZE` chasing exact labels.
pub fn deficit(slots: &[Slot], existing: &[Stored]) -> Vec<Slot> {
    use std::collections::HashMap;
    let mut have: HashMap<Stored, i64> = HashMap::new();
    for s in existing {
        *have.entry(s.clone()).or_insert(0) += 1;
    }
    let mut missing = Vec::new();
    for slot in slots {
        let key: Stored = (slot.group_id.clone(), Some(slot.bloom.clone()), Some(slot.difficulty.clone()), slot.section_id.clone());
        let count = have.entry(key).or_insert(0);
        if *count > 0 {
            *count -= 1;
        } else {
            missing.push(slot.clone());
        }
    }
    let surplus: i64 = have.values().filter(|v| **v > 0).sum();
    let keep = (missing.len() as i64 - surplus).max(0) as usize;
    missing.truncate(keep);
    missing
}

/// Groups missing slots into calls — one call per (group, section) so
/// each can reference exactly the section it's written from, capped at
/// `size` so a truncated reply costs one chunk, not the whole bank.
pub struct Chunk {
    pub group_id: String,
    pub section_id: Option<String>,
    pub slots: Vec<Slot>,
}

pub fn chunks(slots: Vec<Slot>, size: usize) -> Vec<Chunk> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<(String, Option<String>), Vec<Slot>> = BTreeMap::new();
    for slot in slots {
        buckets.entry((slot.group_id.clone(), slot.section_id.clone())).or_default().push(slot);
    }
    let mut out = Vec::new();
    for ((group_id, section_id), group_slots) in buckets {
        for chunk in group_slots.chunks(size) {
            out.push(Chunk { group_id: group_id.clone(), section_id: section_id.clone(), slots: chunk.to_vec() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_blueprint_always_totals_exactly_bank_size() {
        let sections: Vec<String> = (1..=5).map(|i| format!("s{i}")).collect();
        for level in ["SD/MI kelas 4", "SMA/MA kelas 10", "perguruan tinggi S1 Fisika"] {
            for mix in ["hitungan", "konsep", "bahasa"] {
                let slots = bank_blueprint(&sections, level, mix);
                assert_eq!(slots.len() as i64, BANK_SIZE, "level={level} mix={mix}");
            }
        }
    }

    #[test]
    fn bank_blueprint_spreads_every_section_round_robin() {
        let sections: Vec<String> = (1..=7).map(|i| format!("s{i}")).collect();
        let slots = bank_blueprint(&sections, "SMA/MA kelas 10", "hitungan");
        let mut counts = std::collections::HashMap::new();
        for s in &slots {
            *counts.entry(s.section_id.clone()).or_insert(0) += 1;
        }
        assert_eq!(counts.len(), 7, "every section must get at least one slot");
    }

    #[test]
    fn checkpoint_blueprint_is_fixed_easy_to_medium_regardless_of_jenjang() {
        let slots = checkpoint_blueprint("sec1");
        assert_eq!(slots.len() as i64, CHECKPOINT_POOL);
        assert_eq!(slots.iter().map(|s| s.bloom.as_str()).collect::<Vec<_>>(), vec!["c1", "c2", "c2", "c3"]);
        assert!(slots.iter().all(|s| s.subtype == CHECKPOINT_SUBTYPE));
    }

    #[test]
    fn deficit_only_asks_for_what_is_missing_and_ignores_drifted_labels() {
        let slots = vec![
            Slot { group_id: "mc".into(), subtype: "multiple_choice".into(), bloom: "c1".into(), difficulty: "mudah".into(), section_id: Some("s1".into()) },
            Slot { group_id: "mc".into(), subtype: "multiple_choice".into(), bloom: "c2".into(), difficulty: "sedang".into(), section_id: Some("s1".into()) },
        ];
        // First slot already stored; a THIRD question exists that matches
        // no slot at all (a drifted label) and must still count against
        // the plan instead of leaving the bank to grow past its size.
        let existing: Vec<Stored> =
            vec![("mc".into(), Some("c1".into()), Some("mudah".into()), Some("s1".into())), ("mc".into(), Some("c9".into()), Some("mudah".into()), Some("s1".into()))];
        let missing = deficit(&slots, &existing);
        assert!(missing.is_empty(), "the drifted extra question should absorb the still-missing slot: {missing:?}");
    }

    #[test]
    fn chunks_group_by_section_and_cap_the_size() {
        let slots: Vec<Slot> = (0..12).map(|_| Slot { group_id: "mc".into(), subtype: "multiple_choice".into(), bloom: "c1".into(), difficulty: "mudah".into(), section_id: Some("s1".into()) }).collect();
        let c = chunks(slots, 5);
        assert_eq!(c.len(), 3, "12 slots at size 5 -> 5+5+2");
        assert_eq!(c[0].slots.len(), 5);
        assert_eq!(c[2].slots.len(), 2);
    }
}
