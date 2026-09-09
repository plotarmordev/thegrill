use crate::contract::*;
use crate::pack;
use sha2::{Digest, Sha256};
use std::fmt::Write;

const FAMILIES: [&str; 2] = ["ledger", "reachability"];
const GRAPH_SIZE: usize = 8;
const ENVELOPE: &str = "You may reason before answering. End with exactly one line containing ===FINAL===, then one JSON object with only the key \"answer\" and a string value. The answer string must be the integer in ordinary decimal notation (no leading zeros, no plus sign), or that same integer prefixed with value=. Do not add text after the JSON object.";

// Recipe v1: independent SHA-256 parameter block for each (seed, family, unit).
// This is reproducible parameter selection, not a claim of representative sampling.
fn parameters(seed: u64, family: &str, unit: usize) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"the-grill/pilot/v1\0");
    hash.update(seed.to_be_bytes());
    hash.update(family.as_bytes());
    hash.update([0]);
    hash.update((unit as u64).to_be_bytes());
    hash.finalize().into()
}

#[derive(Clone, Copy)]
enum Operation {
    Credit(usize, i32),
    Debit(usize, i32),
    Transfer(usize, usize, i32),
}

struct Ledger {
    initial: [i32; 3],
    operations: [Operation; 9],
    target: usize,
}

impl Ledger {
    fn new(p: &[u8; 32]) -> Self {
        let initial = std::array::from_fn(|i| 10 + i32::from(p[i] % 51));
        let operations = std::array::from_fn(|i| {
            let account = usize::from(p[3 + i] % 3);
            let amount = 1 + i32::from(p[12 + i] % 19);
            match i % 3 {
                0 => Operation::Credit(account, amount),
                1 => Operation::Debit(account, amount),
                _ => Operation::Transfer(
                    account,
                    (account + 1 + usize::from(p[21 + i] % 2)) % 3,
                    amount,
                ),
            }
        });
        Self {
            initial,
            operations,
            target: usize::from(p[31] % 3),
        }
    }

    fn answer(&self) -> i32 {
        let mut balances = self.initial;
        for operation in self.operations {
            match operation {
                Operation::Credit(account, amount) => balances[account] += amount,
                Operation::Debit(account, amount) => balances[account] -= amount,
                Operation::Transfer(from, to, amount) => {
                    balances[from] -= amount;
                    balances[to] += amount;
                }
            }
        }
        balances[self.target]
    }

    fn prompt(&self, variant: usize) -> String {
        let names = if variant == 0 {
            ["amber", "birch", "cedar"]
        } else {
            ["south", "west", "north"]
        };
        let mut prompt = String::from(
            "A ledger has three accounts. Negative balances are permitted. Credit adds to an account; debit subtracts; transfer subtracts from the source and adds the same amount to the destination. Apply every numbered operation once in ascending numeric order, regardless of display order.\nInitial balances:\n",
        );
        for i in 0..3 {
            let account = if variant == 0 { i } else { 2 - i };
            writeln!(prompt, "{}: {}", names[account], self.initial[account]).unwrap();
        }
        prompt.push_str("Operations:\n");
        for i in 0..self.operations.len() {
            let index = if variant == 0 {
                i
            } else {
                self.operations.len() - 1 - i
            };
            write!(prompt, "{}. ", index + 1).unwrap();
            match self.operations[index] {
                Operation::Credit(account, amount) => {
                    writeln!(prompt, "credit {} by {amount}", names[account]).unwrap()
                }
                Operation::Debit(account, amount) => {
                    writeln!(prompt, "debit {} by {amount}", names[account]).unwrap()
                }
                Operation::Transfer(from, to, amount) => writeln!(
                    prompt,
                    "transfer {amount} from {} to {}",
                    names[from], names[to]
                )
                .unwrap(),
            }
        }
        writeln!(
            prompt,
            "What is the final balance of {}?",
            names[self.target]
        )
        .unwrap();
        prompt
    }
}

struct Graph {
    edges: [[bool; GRAPH_SIZE]; GRAPH_SIZE],
}

impl Graph {
    fn new(p: &[u8; 32]) -> Self {
        let mut edges = [[false; GRAPH_SIZE]; GRAPH_SIZE];
        // The last node is isolated. A reachable cycle tests deduplication and
        // exclusion of the start; remaining reachability varies with the seed.
        for (from, row) in edges.iter_mut().enumerate().take(GRAPH_SIZE - 1) {
            for (to, edge) in row.iter_mut().enumerate().take(GRAPH_SIZE - 1) {
                *edge = from != to && (p[(from * 7 + to) % 32] >> (from % 5)) & 7 == 0;
            }
        }
        edges[0][1] = true;
        edges[1][2] = true;
        edges[2][0] = true;
        Self { edges }
    }

    fn answer(&self) -> i32 {
        let mut visited = [false; GRAPH_SIZE];
        let mut queue = [0; GRAPH_SIZE];
        let (mut next, mut end) = (0, 1);
        visited[0] = true;
        while next < end {
            let from = queue[next];
            next += 1;
            for (to, seen) in visited.iter_mut().enumerate() {
                if self.edges[from][to] && !*seen {
                    *seen = true;
                    queue[end] = to;
                    end += 1;
                }
            }
        }
        (end - 1) as i32
    }

    fn prompt(&self, variant: usize) -> String {
        let names = if variant == 0 {
            ["A", "B", "C", "D", "E", "F", "G", "H"]
        } else {
            ["Q", "N", "T", "P", "V", "R", "S", "U"]
        };
        let mut prompt =
            String::from("A directed graph has these nodes (including isolated nodes): ");
        for i in 0..GRAPH_SIZE {
            let node = if variant == 0 { i } else { GRAPH_SIZE - 1 - i };
            if i != 0 {
                prompt.push_str(", ");
            }
            prompt.push_str(names[node]);
        }
        prompt.push_str(".\nThe following list contains all directed edges. An edge X -> Y can be traversed only from X to Y.\n");
        for i in 0..GRAPH_SIZE * GRAPH_SIZE {
            let index = if variant == 0 {
                i
            } else {
                GRAPH_SIZE * GRAPH_SIZE - 1 - i
            };
            let (from, to) = (index / GRAPH_SIZE, index % GRAPH_SIZE);
            if self.edges[from][to] {
                writeln!(prompt, "{} -> {}", names[from], names[to]).unwrap();
            }
        }
        writeln!(prompt, "Starting at {}, how many distinct OTHER nodes can be reached by following one or more edges? Count each node once. Do not count {} even if a cycle reaches it again.", names[0], names[0]).unwrap();
        prompt
    }
}

fn artifact(answer: &str) -> String {
    // Values are generated decimal strings, never externally supplied JSON text.
    format!("===FINAL===\n{{\"answer\":\"{answer}\"}}")
}

fn qualification(answer: i32) -> Qualification {
    let correct = artifact(&answer.to_string());
    let incorrect = artifact(&(answer + 1).to_string());
    Qualification {
        valid: vec![
            correct.clone(),
            artifact(&format!("value={answer}")),
            format!("Work checked.\n{correct}"),
        ],
        wrong: vec![
            incorrect.clone(),
            format!("The number {answer} appeared in my reasoning.\n{incorrect}"),
            format!("{correct}\n{incorrect}"),
            "I cannot answer this question.".into(),
            format!("{{\"answer\":\"{answer}\"}}"),
            format!("===FINAL===\n{{\"answer\":\"{answer}\",\"answer\":\"{answer}\"}}"),
            format!("===FINAL===\n{{\"answer\":{answer}}}"),
            format!("{correct}\nAdditional explanation."),
        ],
    }
}

pub(crate) fn generate(seed: u64, units: usize) -> Result<Pack> {
    if units == 0 || units > CASE_CAP / 4 {
        return Err(format!("pilot units must be in 1..={}", CASE_CAP / 4));
    }
    let mut pack = Pack {
        version: 3,
        label: format!(
            "Pilot recipe v1; seed={seed}; units={units} per family; synthetic diagnostics, not a capability benchmark"
        ),
        worlds: Vec::with_capacity(units * 2),
        groups: FAMILIES.iter().map(|s| (*s).into()).collect(),
        cases: Vec::with_capacity(units * 4),
    };
    for family in FAMILIES {
        for unit in 0..units {
            let world = format!("pilot-v1-{seed}-{family}-{unit}");
            let parameters = parameters(seed, family, unit);
            let (answer, prompts) = if family == "ledger" {
                let ledger = Ledger::new(&parameters);
                (ledger.answer(), [ledger.prompt(0), ledger.prompt(1)])
            } else {
                let graph = Graph::new(&parameters);
                (graph.answer(), [graph.prompt(0), graph.prompt(1)])
            };
            for (variant, prompt) in prompts.into_iter().enumerate() {
                pack.cases.push(Case {
                    id: format!("{world}-v{variant}"),
                    world: world.clone(),
                    group: family.into(),
                    messages: vec![
                        Message {
                            role: Role::System,
                            content: ENVELOPE.into(),
                        },
                        Message {
                            role: Role::User,
                            content: prompt,
                        },
                    ],
                    acceptance: Acceptance::FinalStringSet {
                        accepted: vec![answer.to_string(), format!("value={answer}")],
                    },
                    qualification: qualification(answer),
                    provenance: None,
                });
            }
            pack.worlds.push(world);
        }
    }
    // Exercise the actual wire reader and grader, not an approximate generator check.
    let bytes = serde_json::to_vec(&pack).map_err(|e| format!("pilot encoding: {e}"))?;
    pack::admit(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_oracle_matches_transitive_closure_including_cycles_and_isolation() {
        for seed in 0..128 {
            let graph = Graph::new(&parameters(seed, "reachability", 0));
            let mut closure = graph.edges;
            for via in 0..GRAPH_SIZE {
                for from in 0..GRAPH_SIZE {
                    for to in 0..GRAPH_SIZE {
                        closure[from][to] |= closure[from][via] && closure[via][to];
                    }
                }
            }
            let count = closure[0]
                .iter()
                .enumerate()
                .filter(|(i, edge)| *i != 0 && **edge)
                .count();
            assert_eq!(graph.answer(), count as i32);
            assert!(!closure[0][GRAPH_SIZE - 1]);
        }
    }

    #[test]
    fn ledger_oracle_matches_independent_signed_transaction_sum() {
        for seed in 0..128 {
            let ledger = Ledger::new(&parameters(seed, "ledger", 0));
            let delta: i32 = ledger
                .operations
                .iter()
                .map(|operation| match *operation {
                    Operation::Credit(a, n) => i32::from(a == ledger.target) * n,
                    Operation::Debit(a, n) => -i32::from(a == ledger.target) * n,
                    Operation::Transfer(a, b, n) => {
                        (i32::from(b == ledger.target) - i32::from(a == ledger.target)) * n
                    }
                })
                .sum();
            assert_eq!(ledger.answer(), ledger.initial[ledger.target] + delta);
        }
    }

    #[test]
    fn generation_is_reproducible_and_rejects_unbounded_work() {
        let a = serde_json::to_vec(&generate(u64::MAX, 2).unwrap()).unwrap();
        assert_eq!(
            a,
            serde_json::to_vec(&generate(u64::MAX, 2).unwrap()).unwrap()
        );
        assert_ne!(a, serde_json::to_vec(&generate(0, 2).unwrap()).unwrap());
        assert!(generate(0, 0).is_err());
        assert!(generate(0, usize::MAX).is_err());
    }
}
