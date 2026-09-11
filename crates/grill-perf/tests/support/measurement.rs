#[derive(Clone)]
pub struct Trace {
    pub name: &'static str,
    pub chunks: Vec<(u64, Vec<u8>)>,
}

pub fn traces() -> Vec<Trace> {
    let reasoning = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n";
    let answer = b"data: {\"choices\":[{\"delta\":{\"content\":\"caf\xc3\xa9\"}}]}\r\n\r\n";
    let finish = b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n";
    let usage = b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":8,\"total_tokens\":12}}\n\n";
    let done = b"data: [DONE]\n\n";
    let terminal = [finish.as_slice(), usage.as_slice(), done.as_slice()].concat();
    let paced = vec![
        (10_000, reasoning.to_vec()),
        (30_000, answer.to_vec()),
        (50_000, terminal.clone()),
    ];
    let split = answer.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
    vec![
        Trace { name: "paced", chunks: paced },
        Trace { name: "fragmented", chunks: vec![
            (5_000, reasoning[..reasoning.len() - 1].to_vec()),
            (10_000, reasoning[reasoning.len() - 1..].to_vec()),
            (20_000, answer[..split].to_vec()),
            (30_000, answer[split..].to_vec()),
            (50_000, terminal.clone()),
        ] },
        Trace { name: "coalesced", chunks: vec![(10_000, [reasoning.as_slice(), answer.as_slice(), terminal.as_slice()].concat())] },
        Trace { name: "terminal-tail", chunks: vec![(10_000, reasoning.to_vec()), (30_000, answer.to_vec()), (90_000, terminal)] },
        Trace { name: "missing-usage", chunks: vec![(10_000, reasoning.to_vec()), (30_000, answer.to_vec()), (50_000, [finish.as_slice(), done.as_slice()].concat())] },
        Trace { name: "early-output", chunks: vec![(10_000, reasoning.to_vec()), (30_000, answer.to_vec()), (50_000, [finish.as_slice(), b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":4,\"total_tokens\":8}}\n\n", done.as_slice()].concat())] },
        Trace { name: "error", chunks: vec![(10_000, reasoning.to_vec()), (30_000, b"data: {\"error\":{\"message\":\"synthetic failure\"}}\n\n".to_vec())] },
    ]
}
