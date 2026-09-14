// The paper a learner actually sits, assembled on the server when an
// attempt starts: which questions were drawn from the pool, what order
// they appear in, and how each question's choices were shuffled and
// re-lettered. Stored on `attempts.paper` so grading can translate the
// learner's displayed answers back to the stored keys, and so a refresh
// hands back the identical paper instead of a re-roll.
//
// Everything here is pure (no database), so the draw, shuffle, and
// mapping rules are unit-tested directly. The two non-shuffle cases:
//   * structural — only "flat" groups (one control per question) are
//     drawn from or reordered; a table/flow/word-bank/heading group's
//     order IS its content, so it is kept whole and in place;
//   * content — `choices_fixed` questions keep their choices and letters
//     ("Semua benar", "A dan C", ascending numbers), and `order_locked`
//     questions travel with the question before them as one unit
//     (`quiz_config::detect_order_constraints`).

use std::collections::{BTreeMap, HashMap, HashSet};

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::services::quiz_config::{self, LabeledOption, QuizConfig, QuizQuestion, QuizQuestionGroup};
use crate::services::quiz_subtype::{self, LayoutKind, SubtypeShape};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaperQuestion {
    pub uid: Uuid,
    pub group_id: String,
    /// The answer-map key the learner saw and submits under.
    pub shown_key: String,
    /// The key in the source `quiz_config` the answer is graded against.
    pub original_key: String,
    /// Displayed label → stored label. Empty when choices were not
    /// shuffled (not a choice question, `choices_fixed`, or shuffling off).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub choice_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Paper {
    /// The item whose `quiz_config` holds these questions — the bank for
    /// a "Latihan 10/25 soal" item, the attempted item itself otherwise.
    pub source_item_id: Uuid,
    pub questions: Vec<PaperQuestion>,
    /// Exactly what the learner is shown: drawn questions only, in paper
    /// order, re-lettered, answer keys stripped.
    pub config: Value,
}

pub struct PaperInput<'a> {
    pub source_item_id: Uuid,
    /// Where the questions come from.
    pub source: &'a QuizConfig,
    /// The attempted item's own config — presentation and policy
    /// (title, timing, shuffle flags, `question_pool`) are read from here.
    pub policy: &'a QuizConfig,
    /// Question uids this learner saw on their previous paper from the
    /// same source — drawn last, so a retake mostly brings new questions.
    pub recently_seen: &'a HashSet<Uuid>,
    pub seed: u64,
}

pub fn seed_from(attempt_id: Uuid) -> u64 {
    let n = attempt_id.as_u128();
    (n as u64) ^ ((n >> 64) as u64)
}

fn is_flat(group: &QuizQuestionGroup) -> bool {
    quiz_subtype::find(&group.r#type).is_some_and(|info| matches!(info.layout, LayoutKind::Flat))
}

fn has_letter_choices(group: &QuizQuestionGroup) -> bool {
    quiz_subtype::find(&group.r#type).is_some_and(|info| matches!(info.shape, SubtypeShape::Mcq | SubtypeShape::McqMulti))
}

fn bloom_code(question: &QuizQuestion) -> &'static str {
    question.taxonomy.as_ref().and_then(|t| t.bloom).map(|b| b.code()).unwrap_or("-")
}

/// Consecutive questions joined by `order_locked` — the smallest thing a
/// draw may pick or a shuffle may move.
fn units(questions: &[QuizQuestion]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for (index, question) in questions.iter().enumerate() {
        match out.last_mut() {
            Some(last) if question.order_locked == Some(true) => last.push(index),
            _ => out.push(vec![index]),
        }
    }
    out
}

struct Candidate {
    group_index: usize,
    members: Vec<usize>,
    stratum: (String, &'static str),
    section: Option<String>,
    seen: bool,
}

/// Picks which (group, question-index) pairs make the paper when a pool
/// draws fewer questions than it holds. Stratified by (group, Bloom) so a
/// 10-question draw from a 50-question bank keeps roughly the bank's mix
/// of question types and thinking levels; within a stratum it prefers
/// questions the learner has not just seen, then spreads across source
/// sections round-robin.
fn draw(source: &QuizConfig, draw_count: i64, recently_seen: &HashSet<Uuid>, rng: &mut StdRng) -> HashSet<(usize, usize)> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for (group_index, group) in source.question_groups.iter().enumerate().filter(|(_, g)| is_flat(g)) {
        for members in units(&group.questions) {
            let head = &group.questions[members[0]];
            candidates.push(Candidate {
                group_index,
                stratum: (group.group_id.clone(), bloom_code(head)),
                section: head.source_section_id.clone(),
                seen: members.iter().any(|&i| group.questions[i].uid.is_some_and(|u| recently_seen.contains(&u))),
                members,
            });
        }
    }

    let mut strata: BTreeMap<(String, &'static str), Vec<Candidate>> = BTreeMap::new();
    for candidate in candidates {
        strata.entry(candidate.stratum.clone()).or_default().push(candidate);
    }
    let sizes: Vec<u32> = strata.values().map(|c| c.iter().map(|x| x.members.len() as u32).sum()).collect();
    let mut quotas = crate::services::quiz_taxonomy::apportion(&sizes, draw_count);

    // Freshness over exact proportion, within ±1 per stratum: a stratum
    // that can't fill its quota with unseen questions hands the shortfall
    // to one that can — same question type first. Without this, a
    // stratum that always wins the rounding (a 5/5 bank drawing 5 gives
    // the first stratum 3 every time) repeats a question on every retake
    // while the other stratum still has unseen ones.
    let keys: Vec<(String, &'static str)> = strata.keys().cloned().collect();
    let unseen: Vec<i64> = strata.values().map(|c| c.iter().filter(|x| !x.seen).map(|x| x.members.len() as i64).sum()).collect();
    let original = quotas.clone();
    for i in 0..quotas.len() {
        while quotas[i] > unseen[i] {
            let receiver = (0..quotas.len())
                .filter(|&j| j != i && quotas[j] < unseen[j] && quotas[j] <= original[j])
                .min_by_key(|&j| (keys[j].0 != keys[i].0, j));
            let Some(j) = receiver else { break };
            quotas[i] -= 1;
            quotas[j] += 1;
        }
    }

    let mut picked: HashSet<(usize, usize)> = HashSet::new();
    let mut leftovers: Vec<Candidate> = Vec::new();
    let mut total = 0i64;

    for ((_, mut pool), quota) in strata.into_iter().zip(quotas) {
        pool.shuffle(rng);
        pool.sort_by_key(|c| c.seen); // stable: unseen first, random within
        // Round-robin across source sections so one section can't take
        // the whole quota.
        let mut by_section: BTreeMap<Option<String>, std::collections::VecDeque<Candidate>> = BTreeMap::new();
        let mut section_order: Vec<Option<String>> = Vec::new();
        for candidate in pool {
            if !by_section.contains_key(&candidate.section) {
                section_order.push(candidate.section.clone());
            }
            by_section.entry(candidate.section.clone()).or_default().push_back(candidate);
        }
        let mut taken = 0i64;
        'fill: loop {
            let mut progressed = false;
            for section in &section_order {
                let Some(queue) = by_section.get_mut(section) else { continue };
                let Some(candidate) = queue.front() else { continue };
                if taken + candidate.members.len() as i64 > quota {
                    if taken >= quota {
                        break 'fill;
                    }
                    continue;
                }
                let candidate = queue.pop_front().expect("front just checked");
                taken += candidate.members.len() as i64;
                picked.extend(candidate.members.iter().map(|&m| (candidate.group_index, m)));
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        total += taken;
        leftovers.extend(by_section.into_values().flatten());
    }

    // A stratum that couldn't fill its quota (an oversized chain, or a
    // tiny stratum) hands the shortfall to whatever is left, unseen first.
    if total < draw_count {
        leftovers.shuffle(rng);
        leftovers.sort_by_key(|c| c.seen);
        for candidate in leftovers {
            if total + candidate.members.len() as i64 > draw_count {
                continue;
            }
            total += candidate.members.len() as i64;
            picked.extend(candidate.members.iter().map(|&m| (candidate.group_index, m)));
            if total >= draw_count {
                break;
            }
        }
    }
    picked
}

fn relabel_choices(question: &mut QuizQuestion, rng: &mut StdRng) -> BTreeMap<String, String> {
    const LETTERS: &[u8] = b"ABCDEFGHIJ";
    if question.choices.len() < 2 || question.choices.len() > LETTERS.len() {
        return BTreeMap::new();
    }
    let mut labeled: Vec<(String, LabeledOption)> = question
        .choices
        .iter()
        .enumerate()
        .map(|(i, c)| (c.label().map(str::to_string).unwrap_or_else(|| (LETTERS[i] as char).to_string()), c.clone()))
        .collect();
    labeled.shuffle(rng);
    let mut map = BTreeMap::new();
    question.choices = labeled
        .into_iter()
        .enumerate()
        .map(|(i, (original, option))| {
            let shown = (LETTERS[i] as char).to_string();
            map.insert(shown.clone(), original);
            LabeledOption::Labeled { label: Some(shown), text: option.text().to_string(), image: option.image().map(str::to_string) }
        })
        .collect();
    map
}

pub fn build(input: PaperInput<'_>) -> Paper {
    let mut rng = StdRng::seed_from_u64(input.seed);
    let policy = input.policy;
    let pooled = policy.question_pool.is_some();
    let shuffle_choices = pooled || policy.shuffle_choices == Some(true);
    let shuffle_order = pooled || policy.shuffle_question_order == Some(true);

    let total_flat: i64 = input.source.question_groups.iter().filter(|g| is_flat(g)).map(|g| g.questions.len() as i64).sum();
    let picked = policy
        .question_pool
        .as_ref()
        .filter(|pool| pool.draw_count > 0 && pool.draw_count < total_flat)
        .map(|pool| draw(input.source, pool.draw_count, input.recently_seen, &mut rng));

    let mut groups: Vec<QuizQuestionGroup> = Vec::new();
    let mut maps: Vec<(String, Uuid, String, BTreeMap<String, String>)> = Vec::new(); // (group_id, uid, original_key, choice_map)

    for (group_index, group) in input.source.question_groups.iter().enumerate() {
        let flat = is_flat(group);
        let mut chosen_units: Vec<Vec<usize>> = units(&group.questions)
            .into_iter()
            .filter(|unit| !flat || picked.as_ref().is_none_or(|p| unit.iter().all(|&i| p.contains(&(group_index, i)))))
            .collect();
        if chosen_units.is_empty() {
            continue;
        }
        if flat && shuffle_order {
            chosen_units.shuffle(&mut rng);
        }
        let mut out = group.clone();
        out.questions = Vec::new();
        for index in chosen_units.into_iter().flatten() {
            let mut question = group.questions[index].clone();
            let original_key = question.key();
            let choice_map = if shuffle_choices && has_letter_choices(group) && question.choices_fixed != Some(true) {
                relabel_choices(&mut question, &mut rng)
            } else {
                BTreeMap::new()
            };
            maps.push((group.group_id.clone(), question.uid.unwrap_or_else(Uuid::nil), original_key, choice_map));
            out.questions.push(question);
        }
        groups.push(out);
    }

    if policy.shuffle_questions == Some(true) {
        groups.shuffle(&mut rng);
    }

    // Renumber 1..N in paper order — only when every kept group is flat
    // and every number is a plain single slot. A structural layout's
    // table cells/flow steps point at question numbers, and a "5-6"
    // multi-mark range spans two slots; renumbering either would break
    // the content, so those papers keep their authored numbers.
    let renumber = groups.iter().all(is_flat) && groups.iter().flat_map(|g| &g.questions).all(|q| q.slot_count() == 1);
    let mut shown_keys: HashMap<(String, String), String> = HashMap::new();
    let mut next = 1i64;
    for group in &mut groups {
        for question in &mut group.questions {
            let original = question.key();
            if renumber {
                question.number = Value::from(next);
                next += 1;
            }
            shown_keys.insert((group.group_id.clone(), original), question.key());
        }
    }

    let questions = maps
        .into_iter()
        .map(|(group_id, uid, original_key, choice_map)| PaperQuestion {
            shown_key: shown_keys.get(&(group_id.clone(), original_key.clone())).cloned().unwrap_or_else(|| original_key.clone()),
            uid,
            group_id,
            original_key,
            choice_map,
        })
        .collect();

    let kept_sections: HashSet<&str> = groups.iter().filter_map(|g| g.section_id.as_deref()).collect();
    let mut config = QuizConfig {
        question_groups: groups.clone(),
        sections: input.source.sections.iter().filter(|s| kept_sections.contains(s.section_id.as_str())).cloned().collect(),
        ..policy.clone()
    };
    // The paper is already drawn and shuffled — the client must render it
    // as-is, not reshuffle or redraw it.
    config.question_pool = None;
    config.shuffle_choices = None;
    config.shuffle_question_order = None;
    config.shuffle_questions = None;
    config.ai_generation_meta = None;
    if config.passage.is_none() {
        config.passage = input.source.passage.clone();
    }
    let raw = serde_json::to_value(&config).unwrap_or(Value::Null);

    Paper { source_item_id: input.source_item_id, questions, config: quiz_config::learner_view(&raw) }
}

/// The learner's answer map, re-keyed and re-lettered back onto the
/// source config: displayed "3" → stored "37", displayed "B" → stored
/// "D". Only questions on the paper come through.
pub fn translate_answers(paper: &Paper, answers: &HashMap<String, Value>) -> HashMap<String, Value> {
    let back = |map: &BTreeMap<String, String>, v: &Value| -> Value {
        match v {
            Value::String(s) => Value::String(map.get(s).cloned().unwrap_or_else(|| s.clone())),
            Value::Array(items) => Value::Array(items.iter().map(|i| i.as_str().map(|s| Value::String(map.get(s).cloned().unwrap_or_else(|| s.to_string()))).unwrap_or_else(|| i.clone())).collect()),
            other => other.clone(),
        }
    };
    paper
        .questions
        .iter()
        .filter_map(|q| answers.get(&q.shown_key).map(|v| (q.original_key.clone(), if q.choice_map.is_empty() { v.clone() } else { back(&q.choice_map, v) })))
        .collect()
}

/// A stored answer key expressed in the letters this paper displayed —
/// what the result card shows as "jawaban benar".
pub fn answer_as_shown(paper_question: &PaperQuestion, answer: &Value) -> Value {
    if paper_question.choice_map.is_empty() {
        return answer.clone();
    }
    let forward: HashMap<&str, &str> = paper_question.choice_map.iter().map(|(shown, original)| (original.as_str(), shown.as_str())).collect();
    let map_one = |s: &str| Value::String(forward.get(s).map(|x| x.to_string()).unwrap_or_else(|| s.to_string()));
    match answer {
        Value::String(s) => map_one(s),
        Value::Array(items) => Value::Array(items.iter().map(|i| i.as_str().map(map_one).unwrap_or_else(|| i.clone())).collect()),
        other => other.clone(),
    }
}

/// An explanation written against the stored letters, rewritten to the
/// letters this paper showed. Explanations say "Pilihan C benar karena…";
/// once choices are shuffled and re-lettered, that C is a different
/// choice on the learner's screen, and the explanation contradicts the
/// "Jawaban benar: B" printed right above it. Only letters that follow a
/// cue word ("pilihan", "opsi", "jawaban"), or continue such a list
/// ("pilihan A dan C"), are touched — a lone capital in prose is not.
pub fn explanation_as_shown(paper_question: &PaperQuestion, text: &str) -> String {
    if paper_question.choice_map.is_empty() {
        return text.to_string();
    }
    let forward: HashMap<&str, &str> = paper_question.choice_map.iter().map(|(shown, original)| (original.as_str(), shown.as_str())).collect();
    relabel_letter_references(text, |letter| forward.get(letter).map(|s| s.to_string()))
}

fn relabel_letter_references(text: &str, map: impl Fn(&str) -> Option<String>) -> String {
    const CUES: [&str; 5] = ["pilihan", "opsi", "jawaban", "option", "choice"];
    const SEPARATORS: [&str; 8] = [" dan/atau ", ", dan ", ", atau ", ", ", " dan ", " atau ", " & ", "/"];
    let chars: Vec<char> = text.chars().collect();
    let boundary_before = |j: usize| j == 0 || !chars[j - 1].is_alphanumeric();
    let boundary_after = |j: usize| j >= chars.len() || !chars[j].is_alphanumeric();
    let letter_at = |j: usize| j < chars.len() && chars[j].is_ascii_uppercase() && boundary_before(j) && boundary_after(j + 1);
    let starts_with = |j: usize, needle: &str, fold: bool| {
        let n = needle.chars().count();
        j + n <= chars.len() && {
            let got: String = chars[j..j + n].iter().collect();
            if fold { got.to_lowercase() == needle } else { got == needle }
        }
    };

    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let cue = if boundary_before(i) { CUES.iter().find(|c| starts_with(i, c, true) && boundary_after(i + c.chars().count())) } else { None };
        let Some(cue) = cue else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let cue_end = i + cue.chars().count();
        out.extend(&chars[i..cue_end]);
        let mut j = cue_end;
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j == cue_end || !letter_at(j) {
            i = cue_end;
            continue;
        }
        out.extend(&chars[cue_end..j]);
        loop {
            let letter = chars[j].to_string();
            out.push_str(&map(&letter).unwrap_or(letter));
            j += 1;
            match SEPARATORS.iter().find(|sep| starts_with(j, sep, false) && letter_at(j + sep.chars().count())) {
                Some(sep) => {
                    let next = j + sep.chars().count();
                    out.extend(&chars[j..next]);
                    j = next;
                }
                None => break,
            }
        }
        i = j;
    }
    out
}

pub fn original_key_lookup(paper: &Paper) -> HashMap<String, &PaperQuestion> {
    paper.questions.iter().map(|q| (q.original_key.clone(), q)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explanation_letters_follow_the_shuffled_choices() {
        // Shown B was stored C, shown C was stored A.
        let pq = PaperQuestion { uid: Uuid::nil(), group_id: "mc".into(), shown_key: "2".into(), original_key: "7".into(), choice_map: [("A", "B"), ("B", "C"), ("C", "A")].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() };
        assert_eq!(
            explanation_as_shown(&pq, "Pilihan C benar karena utuh. Pilihan A salah, begitu juga opsi B."),
            "Pilihan B benar karena utuh. Pilihan C salah, begitu juga opsi A."
        );
        assert_eq!(explanation_as_shown(&pq, "Jawaban A dan C keliru."), "Jawaban C dan B keliru.");
        // A capital that is not a choice reference is left alone.
        assert_eq!(explanation_as_shown(&pq, "Titik A ada di kiri, pilihannya C."), "Titik A ada di kiri, pilihannya C.");
        let unshuffled = PaperQuestion { choice_map: BTreeMap::new(), ..pq };
        assert_eq!(explanation_as_shown(&unshuffled, "Pilihan C benar."), "Pilihan C benar.");
    }

    use crate::services::quiz_config::QuestionPool;
    use serde_json::json;

    fn bank(mc: usize, tf: usize) -> QuizConfig {
        let mut number = 0;
        let mut q = |stem: String, bloom: &str, section: &str, choices: bool| {
            number += 1;
            let mut v = json!({"number": number, "uid": Uuid::new_v4(), "stem": stem, "answer": "A", "taxonomy": {"bloom": bloom}, "source_section_id": section});
            if choices {
                v["choices"] = json!([{"label": "A", "text": "benar"}, {"label": "B", "text": "salah1"}, {"label": "C", "text": "salah2"}, {"label": "D", "text": "salah3"}]);
            }
            v
        };
        let blooms = ["c1", "c2", "c2", "c3", "c4"];
        let mc_qs: Vec<Value> = (0..mc).map(|i| q(format!("mc {i}"), blooms[i % 5], ["s1", "s2", "s3", "s4", "s5"][i % 5], true)).collect();
        let tf_qs: Vec<Value> = (0..tf).map(|i| q(format!("tf {i}"), "c2", ["s1", "s2"][i % 2], false)).collect();
        serde_json::from_value(json!({"question_groups": [
            {"group_id": "mc", "type": "multiple_choice", "questions": mc_qs},
            {"group_id": "tf", "type": "true_false", "questions": tf_qs}
        ]}))
        .unwrap()
    }

    fn paper(source: &QuizConfig, draw: Option<i64>, seed: u64) -> Paper {
        let mut policy = QuizConfig::default();
        policy.question_pool = draw.map(|n| QuestionPool { source_item_id: None, draw_count: n });
        build(PaperInput { source_item_id: Uuid::nil(), source, policy: &policy, recently_seen: &HashSet::new(), seed })
    }

    #[test]
    fn a_draw_takes_exactly_the_count_and_keeps_the_banks_mix() {
        let source = bank(40, 10);
        let p = paper(&source, Some(10), 7);
        assert_eq!(p.questions.len(), 10);
        let tf = p.questions.iter().filter(|q| q.group_id == "tf").count();
        assert_eq!(tf, 2, "10 of 50 from a 40/10 bank should carry 2 true/false");
        let shown: Vec<_> = p.questions.iter().map(|q| q.shown_key.as_str()).collect();
        let mut sorted = shown.clone();
        sorted.sort_by_key(|k| k.parse::<i64>().unwrap());
        assert_eq!(sorted, (1..=10).map(|n| n.to_string()).collect::<Vec<_>>(), "a paper is renumbered 1..N");
    }

    #[test]
    fn two_seeds_give_different_papers_and_the_same_seed_the_same_paper() {
        let source = bank(40, 10);
        assert_eq!(paper(&source, Some(10), 1), paper(&source, Some(10), 1));
        assert_ne!(paper(&source, Some(10), 1).questions, paper(&source, Some(10), 2).questions);
    }

    #[test]
    fn a_retake_prefers_questions_not_seen_last_time() {
        let source = bank(20, 0);
        let first = paper(&source, Some(10), 3);
        let seen: HashSet<Uuid> = first.questions.iter().map(|q| q.uid).collect();
        let mut policy = QuizConfig::default();
        policy.question_pool = Some(QuestionPool { source_item_id: None, draw_count: 10 });
        let second = build(PaperInput { source_item_id: Uuid::nil(), source: &source, policy: &policy, recently_seen: &seen, seed: 4 });
        let overlap = second.questions.iter().filter(|q| seen.contains(&q.uid)).count();
        assert_eq!(overlap, 0, "20 questions, 10 seen — a 10-question retake can be entirely new");
    }

    #[test]
    fn shuffled_choices_are_relettered_and_answers_translate_back() {
        let source = bank(5, 0);
        let p = paper(&source, Some(50), 11);
        let q = &p.questions[0];
        assert_eq!(q.choice_map.len(), 4);
        let shown_config_q = p.config["question_groups"][0]["questions"][0].clone();
        let labels: Vec<_> = shown_config_q["choices"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap().to_string()).collect();
        assert_eq!(labels, vec!["A", "B", "C", "D"], "letters follow position, never C, A, D, B");
        assert!(shown_config_q.get("answer").is_none(), "the paper must not carry the key");

        // Whichever displayed letter stands for stored "A" is the right one.
        let right_shown = q.choice_map.iter().find(|(_, original)| *original == "A").unwrap().0.clone();
        let mut answers = HashMap::new();
        answers.insert(q.shown_key.clone(), json!(right_shown));
        let translated = translate_answers(&p, &answers);
        assert_eq!(translated.get(&q.original_key), Some(&json!("A")));
        assert_eq!(answer_as_shown(q, &json!("A")), json!(right_shown));
    }

    #[test]
    fn fixed_choices_and_locked_chains_are_respected() {
        let mut source = bank(6, 0);
        source.question_groups[0].questions[0].choices_fixed = Some(true);
        source.question_groups[0].questions[3].order_locked = Some(true); // leans on #3
        let p = paper(&source, Some(50), 5);
        let first_uid = source.question_groups[0].questions[0].uid.unwrap();
        assert!(p.questions.iter().find(|q| q.uid == first_uid).unwrap().choice_map.is_empty());

        let order: Vec<Uuid> = p.questions.iter().map(|q| q.uid).collect();
        let a = order.iter().position(|u| *u == source.question_groups[0].questions[2].uid.unwrap()).unwrap();
        let b = order.iter().position(|u| *u == source.question_groups[0].questions[3].uid.unwrap()).unwrap();
        assert_eq!(b, a + 1, "a locked question stays right after the one it leans on");
    }

    #[test]
    fn a_non_flat_group_is_kept_whole_and_keeps_its_numbers() {
        let source: QuizConfig = serde_json::from_value(json!({"question_groups": [
            {"group_id": "t", "type": "table_completion", "columns": ["a"], "rows": [{"cells": [{"question_number": 7}]}],
             "questions": [{"number": 7, "uid": Uuid::new_v4(), "answer": "x"}, {"number": 8, "uid": Uuid::new_v4(), "answer": "y"}]}
        ]}))
        .unwrap();
        let mut policy = QuizConfig::default();
        policy.shuffle_question_order = Some(true);
        let p = build(PaperInput { source_item_id: Uuid::nil(), source: &source, policy: &policy, recently_seen: &HashSet::new(), seed: 9 });
        let keys: Vec<_> = p.questions.iter().map(|q| q.shown_key.as_str()).collect();
        assert_eq!(keys, vec!["7", "8"], "table cells point at 7 and 8 — neither reordered nor renumbered");
    }
}
