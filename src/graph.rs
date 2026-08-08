use crate::{
    segment::{PassageKind, SEGMENTER_VERSION, SegmentError, segment_document},
    tokenize::tokenize_for_index_with_positions,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentType {
    Page,
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentGraphInput<'a> {
    pub document_type: DocumentType,
    pub identifier: &'a str,
    pub label: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalNode {
    pub node_id: String,
    pub node_type: &'static str,
    pub label: String,
    pub document_type: Option<DocumentType>,
    pub document_identifier: Option<String>,
    pub parent_node_id: Option<String>,
    pub ordinal: Option<usize>,
    pub byte_range: Option<Range<usize>>,
    pub content_fingerprint: Option<String>,
    pub segmenter_version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEdge {
    pub edge_id: String,
    pub edge_type: &'static str,
    pub from_node_id: String,
    pub to_node_id: String,
    pub frequency: Option<usize>,
    pub positions: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentGraphReplacement {
    pub document_node_id: String,
    pub content_fingerprint: String,
    pub nodes: Vec<CanonicalNode>,
    pub edges: Vec<CanonicalEdge>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TermPairContribution {
    pub from_term_id: String,
    pub to_term_id: String,
    pub sentence_weight: f64,
    pub passage_weight: f64,
    pub witness_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CooccurrenceBuild {
    pub contributions: Vec<TermPairContribution>,
    pub truncated_sentence_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedCooccurrence {
    pub from_term_id: String,
    pub to_term_id: String,
    pub normalized_strength: f64,
    pub raw_strength: f64,
    pub witness_count: usize,
    pub rank: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphDelta {
    pub action: &'static str,
    pub entity_type: &'static str,
    pub entity_id: String,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
}

pub fn canonical_graph_digest(graph: &DocumentGraphReplacement) -> String {
    let mut hasher = Sha256::new();
    for ((entity_type, entity_id), record) in graph_records(graph) {
        for value in [entity_type, entity_id, record] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
    }
    hex_digest(hasher.finalize())
}

pub fn diff_document_graph(
    before: Option<&DocumentGraphReplacement>,
    after: Option<&DocumentGraphReplacement>,
) -> Vec<GraphDelta> {
    let before = before.map(graph_records).unwrap_or_default();
    let after = after.map(graph_records).unwrap_or_default();
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .filter_map(|(entity_type, entity_id)| {
            let old = before.get(&(entity_type.clone(), entity_id.clone()));
            let new = after.get(&(entity_type.clone(), entity_id.clone()));
            if old == new {
                return None;
            }
            Some(GraphDelta {
                action: match (old, new) {
                    (None, Some(_)) => "add",
                    (Some(_), None) => "remove",
                    (Some(_), Some(_)) => "update",
                    (None, None) => unreachable!(),
                },
                entity_type: if entity_type == "node" {
                    "node"
                } else {
                    "edge"
                },
                entity_id,
                before_json: old.cloned(),
                after_json: new.cloned(),
            })
        })
        .collect()
}

fn graph_records(graph: &DocumentGraphReplacement) -> BTreeMap<(String, String), String> {
    let mut records = BTreeMap::new();
    for node in &graph.nodes {
        let document_type = node.document_type.map(document_type_name);
        let byte_range = node
            .byte_range
            .as_ref()
            .map(|range| [range.start, range.end]);
        records.insert(
            ("node".to_string(), node.node_id.clone()),
            json!({
                "byte_range": byte_range,
                "content_fingerprint": node.content_fingerprint,
                "document_identifier": node.document_identifier,
                "document_type": document_type,
                "label": node.label,
                "node_id": node.node_id,
                "node_type": node.node_type,
                "ordinal": node.ordinal,
                "parent_node_id": node.parent_node_id,
                "segmenter_version": node.segmenter_version,
            })
            .to_string(),
        );
    }
    for edge in &graph.edges {
        let positions = edge
            .positions
            .iter()
            .map(|range| [range.start, range.end])
            .collect::<Vec<_>>();
        records.insert(
            ("edge".to_string(), edge.edge_id.clone()),
            json!({
                "edge_id": edge.edge_id,
                "edge_type": edge.edge_type,
                "frequency": edge.frequency,
                "from_node_id": edge.from_node_id,
                "positions": positions,
                "to_node_id": edge.to_node_id,
            })
            .to_string(),
        );
    }
    records
}

fn document_type_name(document_type: DocumentType) -> &'static str {
    match document_type {
        DocumentType::Page => "page",
        DocumentType::Source => "source",
    }
}

pub fn rank_cooccurrence(
    contributions: &[TermPairContribution],
    limit: usize,
) -> Vec<RankedCooccurrence> {
    let mut pairs: BTreeMap<(String, String), (f64, usize)> = BTreeMap::new();
    for contribution in contributions {
        let total = pairs
            .entry((
                contribution.from_term_id.clone(),
                contribution.to_term_id.clone(),
            ))
            .or_default();
        total.0 += contribution.sentence_weight + contribution.passage_weight;
        total.1 += contribution.witness_count;
    }
    let mut masses: BTreeMap<String, f64> = BTreeMap::new();
    for ((from, _), (raw_strength, _)) in &pairs {
        *masses.entry(from.clone()).or_default() += raw_strength;
    }
    let mut grouped: BTreeMap<String, Vec<RankedCooccurrence>> = BTreeMap::new();
    for ((from, to), (raw_strength, witness_count)) in pairs {
        let denominator = masses.get(&from).copied().unwrap_or_default()
            + masses.get(&to).copied().unwrap_or_default();
        let normalized_strength = if denominator > 0.0 {
            (2.0 * raw_strength / denominator).clamp(0.0, 1.0)
        } else {
            0.0
        };
        grouped
            .entry(from.clone())
            .or_default()
            .push(RankedCooccurrence {
                from_term_id: from,
                to_term_id: to,
                normalized_strength,
                raw_strength,
                witness_count,
                rank: 0,
            });
    }

    let mut ranked = Vec::new();
    for (_, mut neighbors) in grouped {
        neighbors.sort_by(|left, right| {
            right
                .normalized_strength
                .total_cmp(&left.normalized_strength)
                .then_with(|| right.witness_count.cmp(&left.witness_count))
                .then_with(|| left.to_term_id.cmp(&right.to_term_id))
        });
        neighbors.truncate(limit);
        for (index, neighbor) in neighbors.iter_mut().enumerate() {
            neighbor.rank = index + 1;
        }
        ranked.extend(neighbors);
    }
    ranked
}

pub fn build_cooccurrence(
    input: &DocumentGraphInput<'_>,
) -> Result<CooccurrenceBuild, SegmentError> {
    const WINDOW: usize = 12;
    const MAX_SENTENCE_TOKENS: usize = 512;
    const MAX_PASSAGE_TOKENS: usize = 4_096;

    let segmented = segment_document(input.content)?;
    let mut totals: BTreeMap<(String, String), (f64, f64, usize)> = BTreeMap::new();
    let mut truncated_sentence_count = 0usize;

    for passage in segmented.passages {
        let mut passage_remaining = MAX_PASSAGE_TOKENS;
        let mut sentences = Vec::with_capacity(passage.sentences.len());
        for sentence in passage.sentences {
            let mut tokens = tokenize_for_index_with_positions(&input.content[sentence.range]);
            let limit = MAX_SENTENCE_TOKENS.min(passage_remaining);
            if tokens.len() > limit {
                tokens.truncate(limit);
                truncated_sentence_count += 1;
            }
            passage_remaining -= tokens.len();
            sentences.push(
                tokens
                    .into_iter()
                    .map(|token| format!("term:{}", token.normalized))
                    .collect::<Vec<_>>(),
            );
        }

        for tokens in &sentences {
            for left in 0..tokens.len() {
                for right in (left + 1)..tokens.len().min(left + WINDOW + 1) {
                    if tokens[left] == tokens[right] {
                        continue;
                    }
                    let weight = 1.0 / (1.0 + (right - left) as f64);
                    add_pair_weight(&mut totals, &tokens[left], &tokens[right], weight, 0.0);
                }
            }
        }

        for pair in sentences.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            let left_start = left.len().saturating_sub(WINDOW);
            for left_index in left_start..left.len() {
                for (right_index, right_term) in right.iter().take(WINDOW).enumerate() {
                    if left[left_index] == *right_term {
                        continue;
                    }
                    let distance = left.len() - left_index + right_index;
                    let weight = 0.25 / (1.0 + distance as f64);
                    add_pair_weight(&mut totals, &left[left_index], right_term, 0.0, weight);
                }
            }
        }
    }

    Ok(CooccurrenceBuild {
        contributions: totals
            .into_iter()
            .map(
                |((from_term_id, to_term_id), (sentence_weight, passage_weight, witness_count))| {
                    TermPairContribution {
                        from_term_id,
                        to_term_id,
                        sentence_weight,
                        passage_weight,
                        witness_count,
                    }
                },
            )
            .collect(),
        truncated_sentence_count,
    })
}

fn add_pair_weight(
    totals: &mut BTreeMap<(String, String), (f64, f64, usize)>,
    left: &str,
    right: &str,
    sentence_weight: f64,
    passage_weight: f64,
) {
    for (from, to) in [(left, right), (right, left)] {
        let total = totals
            .entry((from.to_string(), to.to_string()))
            .or_default();
        total.0 += sentence_weight;
        total.1 += passage_weight;
        total.2 += 1;
    }
}

pub fn build_document_graph(
    input: &DocumentGraphInput<'_>,
) -> Result<DocumentGraphReplacement, SegmentError> {
    let document_node_id = format!(
        "{}:{}",
        match input.document_type {
            DocumentType::Page => "page",
            DocumentType::Source => "source",
        },
        input.identifier
    );
    let content_fingerprint = sha256_hex(input.content.as_bytes());
    let segmented = segment_document(input.content)?;
    let mut nodes = vec![CanonicalNode {
        node_id: document_node_id.clone(),
        node_type: "document",
        label: input.label.to_string(),
        document_type: Some(input.document_type),
        document_identifier: Some(input.identifier.to_string()),
        parent_node_id: None,
        ordinal: None,
        byte_range: None,
        content_fingerprint: Some(content_fingerprint.clone()),
        segmenter_version: Some(SEGMENTER_VERSION),
    }];
    let mut edges = Vec::new();
    let mut passage_ids = Vec::with_capacity(segmented.passages.len());
    let mut term_labels = BTreeSet::new();
    let mut postings: BTreeMap<(String, String), Vec<Range<usize>>> = BTreeMap::new();

    for passage in &segmented.passages {
        let passage_id = span_id(
            &document_node_id,
            &content_fingerprint,
            passage_kind_name(passage.kind),
            passage.ordinal,
            &passage.range,
        );
        passage_ids.push(passage_id.clone());
        nodes.push(span_node(
            passage_id.clone(),
            "passage",
            input,
            &document_node_id,
            passage.ordinal,
            passage.range.clone(),
            &content_fingerprint,
        ));
        edges.push(automatic_edge("CONTAINS", &document_node_id, &passage_id));

        let mut sentence_ids = Vec::with_capacity(passage.sentences.len());
        for sentence in &passage.sentences {
            let sentence_id = span_id(
                &document_node_id,
                &content_fingerprint,
                "sentence",
                sentence.ordinal,
                &sentence.range,
            );
            sentence_ids.push(sentence_id.clone());
            nodes.push(span_node(
                sentence_id.clone(),
                "sentence",
                input,
                &passage_id,
                sentence.ordinal,
                sentence.range.clone(),
                &content_fingerprint,
            ));
            edges.push(automatic_edge("CONTAINS", &passage_id, &sentence_id));

            for occurrence in
                tokenize_for_index_with_positions(&input.content[sentence.range.clone()])
            {
                let absolute = (sentence.range.start + occurrence.byte_start)
                    ..(sentence.range.start + occurrence.byte_end);
                let term_id = format!("term:{}", occurrence.normalized);
                term_labels.insert(occurrence.normalized);
                postings
                    .entry((term_id, sentence_id.clone()))
                    .or_default()
                    .push(absolute);
            }
        }
        for occurrence in tokenize_for_index_with_positions(&input.content[passage.range.clone()]) {
            let absolute = (passage.range.start + occurrence.byte_start)
                ..(passage.range.start + occurrence.byte_end);
            let term_id = format!("term:{}", occurrence.normalized);
            term_labels.insert(occurrence.normalized);
            postings
                .entry((term_id, passage_id.clone()))
                .or_default()
                .push(absolute);
        }
        add_peer_edges(&mut edges, &sentence_ids);
    }
    for occurrence in tokenize_for_index_with_positions(input.content) {
        let term_id = format!("term:{}", occurrence.normalized);
        term_labels.insert(occurrence.normalized);
        postings
            .entry((term_id, document_node_id.clone()))
            .or_default()
            .push(occurrence.byte_start..occurrence.byte_end);
    }
    add_peer_edges(&mut edges, &passage_ids);

    for term in term_labels {
        nodes.push(CanonicalNode {
            node_id: format!("term:{term}"),
            node_type: "term",
            label: term,
            document_type: None,
            document_identifier: None,
            parent_node_id: None,
            ordinal: None,
            byte_range: None,
            content_fingerprint: None,
            segmenter_version: None,
        });
    }
    for ((term_id, target_id), mut positions) in postings {
        positions.sort_by_key(|range| (range.start, range.end));
        positions.dedup();
        if positions.len() > 65_536 {
            return Err(SegmentError::TooManyPositions {
                limit: 65_536,
                actual: positions.len(),
            });
        }
        edges.push(CanonicalEdge {
            edge_id: stable_id("edge", &["OCCURS_IN", &term_id, &target_id]),
            edge_type: "OCCURS_IN",
            from_node_id: term_id,
            to_node_id: target_id,
            frequency: Some(positions.len()),
            positions,
        });
    }

    Ok(DocumentGraphReplacement {
        document_node_id,
        content_fingerprint,
        nodes,
        edges,
    })
}

fn passage_kind_name(kind: PassageKind) -> &'static str {
    match kind {
        PassageKind::Paragraph => "paragraph",
        PassageKind::Heading => "heading",
        PassageKind::ListItem => "list-item",
        PassageKind::BlockQuote => "block-quote",
        PassageKind::TableRow => "table-row",
        PassageKind::CodeBlock => "code-block",
        PassageKind::Html => "html",
        PassageKind::Fallback => "fallback",
    }
}

fn span_node(
    node_id: String,
    node_type: &'static str,
    input: &DocumentGraphInput<'_>,
    parent_node_id: &str,
    ordinal: usize,
    byte_range: Range<usize>,
    content_fingerprint: &str,
) -> CanonicalNode {
    CanonicalNode {
        node_id,
        node_type,
        label: input.content[byte_range.clone()].to_string(),
        document_type: Some(input.document_type),
        document_identifier: Some(input.identifier.to_string()),
        parent_node_id: Some(parent_node_id.to_string()),
        ordinal: Some(ordinal),
        byte_range: Some(byte_range),
        content_fingerprint: Some(content_fingerprint.to_string()),
        segmenter_version: Some(SEGMENTER_VERSION),
    }
}

fn span_id(
    document_node_id: &str,
    content_fingerprint: &str,
    kind: &str,
    ordinal: usize,
    range: &Range<usize>,
) -> String {
    stable_id(
        "span",
        &[
            document_node_id,
            content_fingerprint,
            &SEGMENTER_VERSION.to_string(),
            kind,
            &ordinal.to_string(),
            &range.start.to_string(),
            &range.end.to_string(),
        ],
    )
}

pub fn automatic_edge(
    edge_type: &'static str,
    from_node_id: &str,
    to_node_id: &str,
) -> CanonicalEdge {
    CanonicalEdge {
        edge_id: stable_id("edge", &[edge_type, from_node_id, to_node_id]),
        edge_type,
        from_node_id: from_node_id.to_string(),
        to_node_id: to_node_id.to_string(),
        frequency: None,
        positions: Vec::new(),
    }
}

fn add_peer_edges(edges: &mut Vec<CanonicalEdge>, node_ids: &[String]) {
    for pair in node_ids.windows(2) {
        edges.push(automatic_edge("NEXT", &pair[0], &pair[1]));
        edges.push(automatic_edge("PREVIOUS", &pair[1], &pair[0]));
    }
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{prefix}:{}", hex_digest(hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher.finalize())
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphPage {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub source_ids: Vec<i64>,
    pub outlinks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelatedPage {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub direct_link_score: f64,
    pub shared_source_score: f64,
    pub common_neighbor_score: f64,
    pub type_affinity_score: f64,
    pub total_score: f64,
}

pub fn related(seed: &GraphPage, pages: &[GraphPage], limit: usize) -> Vec<RelatedPage> {
    let seed_slug = normalize_key(&seed.slug);
    let mut page_map = HashMap::new();
    let mut outlinks_map = HashMap::new();
    let mut inlinks_map: HashMap<String, HashSet<String>> = HashMap::new();

    page_map.insert(seed_slug.clone(), seed);
    outlinks_map.insert(seed_slug.clone(), normalized_outlinks(seed));
    inlinks_map.entry(seed_slug.clone()).or_default();

    for page in pages {
        let slug = normalize_key(&page.slug);
        page_map.entry(slug.clone()).or_insert(page);
        outlinks_map.insert(slug.clone(), normalized_outlinks(page));
        inlinks_map.entry(slug).or_default();
    }

    for (from_slug, outlinks) in &outlinks_map {
        for to_slug in outlinks {
            if page_map.contains_key(to_slug) {
                inlinks_map
                    .entry(to_slug.clone())
                    .or_default()
                    .insert(from_slug.clone());
            }
        }
    }

    let seed_outlinks = outlinks_map.get(&seed_slug).cloned().unwrap_or_default();
    let seed_inlinks = inlinks_map.get(&seed_slug).cloned().unwrap_or_default();
    let seed_neighbors = neighbors(&seed_outlinks, &seed_inlinks);
    let seed_sources = normalized_sources(seed);
    let seed_kind = normalized_kind(seed.kind.as_deref());

    let mut results = Vec::new();
    for page in pages {
        let candidate_slug = normalize_key(&page.slug);
        if candidate_slug == seed_slug {
            continue;
        }

        let candidate_outlinks = outlinks_map
            .get(&candidate_slug)
            .cloned()
            .unwrap_or_default();
        let candidate_inlinks = inlinks_map
            .get(&candidate_slug)
            .cloned()
            .unwrap_or_default();
        let candidate_neighbors = neighbors(&candidate_outlinks, &candidate_inlinks);

        let direct_link_score = direct_link_score(
            &seed_slug,
            &candidate_slug,
            &seed_outlinks,
            &candidate_outlinks,
        );
        let shared_source_score = shared_source_score(&seed_sources, &normalized_sources(page));
        let common_neighbor_score = common_neighbor_score(
            &seed_neighbors,
            &candidate_neighbors,
            &outlinks_map,
            &inlinks_map,
        );
        let structural_score = direct_link_score + shared_source_score + common_neighbor_score;
        if structural_score <= 0.0 {
            continue;
        }
        let type_affinity_score =
            type_affinity_score(&seed_kind, &normalized_kind(page.kind.as_deref()));
        let total_score = structural_score + type_affinity_score;

        results.push(RelatedPage {
            slug: page.slug.clone(),
            title: page.title.clone(),
            kind: page.kind.clone(),
            direct_link_score,
            shared_source_score,
            common_neighbor_score,
            type_affinity_score,
            total_score,
        });
    }

    results.sort_by(|left, right| {
        right
            .total_score
            .total_cmp(&left.total_score)
            .then_with(|| left.slug.cmp(&right.slug))
    });
    results.truncate(limit);
    results
}

fn normalized_outlinks(page: &GraphPage) -> HashSet<String> {
    page.outlinks
        .iter()
        .map(|link| normalize_key(link))
        .collect()
}

fn normalized_sources(page: &GraphPage) -> HashSet<i64> {
    page.source_ids.iter().copied().collect()
}

fn neighbors(outlinks: &HashSet<String>, inlinks: &HashSet<String>) -> HashSet<String> {
    outlinks
        .iter()
        .chain(inlinks.iter())
        .cloned()
        .collect::<HashSet<_>>()
}

fn direct_link_score(
    seed_slug: &str,
    candidate_slug: &str,
    seed_outlinks: &HashSet<String>,
    candidate_outlinks: &HashSet<String>,
) -> f64 {
    let mut score = 0.0;
    if seed_outlinks.contains(candidate_slug) {
        score += 3.0;
    }
    if candidate_outlinks.contains(seed_slug) {
        score += 3.0;
    }
    score
}

fn shared_source_score(seed_sources: &HashSet<i64>, candidate_sources: &HashSet<i64>) -> f64 {
    let shared_count = seed_sources.intersection(candidate_sources).count();
    shared_count as f64 * 4.0
}

fn common_neighbor_score(
    seed_neighbors: &HashSet<String>,
    candidate_neighbors: &HashSet<String>,
    outlinks_map: &HashMap<String, HashSet<String>>,
    inlinks_map: &HashMap<String, HashSet<String>>,
) -> f64 {
    let mut adamic_adar = 0.0;
    for neighbor in seed_neighbors.intersection(candidate_neighbors) {
        let degree = outlinks_map.get(neighbor).map_or(0, HashSet::len)
            + inlinks_map.get(neighbor).map_or(0, HashSet::len);
        let bounded_degree = degree.max(2) as f64;
        adamic_adar += 1.0 / bounded_degree.ln();
    }
    adamic_adar * 1.5
}

fn type_affinity_score(seed_kind: &str, candidate_kind: &str) -> f64 {
    let affinity = match seed_kind {
        "entity" => match candidate_kind {
            "concept" => 1.2,
            "entity" => 0.8,
            "source" => 1.0,
            "synthesis" => 1.0,
            "query" => 0.8,
            _ => 0.5,
        },
        "concept" => match candidate_kind {
            "entity" => 1.2,
            "concept" => 0.8,
            "source" => 1.0,
            "synthesis" => 1.2,
            "query" => 1.0,
            _ => 0.5,
        },
        "source" => match candidate_kind {
            "entity" => 1.0,
            "concept" => 1.0,
            "source" => 0.5,
            "query" => 0.8,
            "synthesis" => 1.0,
            _ => 0.5,
        },
        "query" => match candidate_kind {
            "concept" => 1.0,
            "entity" => 0.8,
            "synthesis" => 1.0,
            "source" => 0.8,
            "query" => 0.5,
            _ => 0.5,
        },
        "synthesis" => match candidate_kind {
            "concept" => 1.2,
            "entity" => 1.0,
            "source" => 1.0,
            "query" => 1.0,
            "synthesis" => 0.8,
            _ => 0.5,
        },
        _ => 0.5,
    };
    affinity * 1.0
}

fn normalize_key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn normalized_kind(kind: Option<&str>) -> String {
    kind.map(normalize_key)
        .filter(|kind| !kind.is_empty())
        .unwrap_or_else(|| "other".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        DocumentGraphInput, DocumentType, GraphPage, TermPairContribution, build_cooccurrence,
        build_document_graph, canonical_graph_digest, diff_document_graph, rank_cooccurrence,
        related,
    };
    use crate::segment::SegmentError;

    fn page(
        slug: &str,
        title: &str,
        kind: Option<&str>,
        source_ids: &[i64],
        outlinks: &[&str],
    ) -> GraphPage {
        GraphPage {
            slug: slug.to_string(),
            title: title.to_string(),
            kind: kind.map(str::to_string),
            source_ids: source_ids.to_vec(),
            outlinks: outlinks.iter().map(|link| (*link).to_string()).collect(),
        }
    }

    fn approx_eq(left: f64, right: f64) {
        let delta = (left - right).abs();
        assert!(
            delta < 1e-9,
            "expected {left} to equal {right} within tolerance, delta={delta}"
        );
    }

    #[test]
    fn counts_bidirectional_direct_links() {
        let seed = page("seed", "Seed", Some("entity"), &[], &["candidate"]);
        let candidate = page("candidate", "Candidate", Some("source"), &[], &["seed"]);

        let results = related(&seed, &[candidate], 10);
        assert_eq!(results.len(), 1);
        approx_eq(results[0].direct_link_score, 6.0);
        approx_eq(results[0].type_affinity_score, 1.0);
        approx_eq(results[0].total_score, 7.0);
    }

    #[test]
    fn counts_shared_sources() {
        let seed = page("seed", "Seed", Some("entity"), &[1, 2, 3], &[]);
        let candidate = page("candidate", "Candidate", Some("entity"), &[2, 3, 9], &[]);

        let results = related(&seed, &[candidate], 10);
        assert_eq!(results.len(), 1);
        approx_eq(results[0].shared_source_score, 8.0);
        approx_eq(results[0].type_affinity_score, 0.8);
        approx_eq(results[0].total_score, 8.8);
    }

    #[test]
    fn counts_common_neighbors_with_adamic_adar() {
        let seed = page("seed", "Seed", None, &[], &["hub"]);
        let candidate = page("candidate", "Candidate", None, &[], &["hub"]);
        let hub = page("hub", "Hub", None, &[], &[]);

        let results = related(&seed, &[candidate, hub], 10);
        assert_eq!(results.len(), 2);
        let candidate_result = results
            .into_iter()
            .find(|result| result.slug == "candidate")
            .unwrap();
        approx_eq(candidate_result.common_neighbor_score, 1.5 / 2.0_f64.ln());
        approx_eq(candidate_result.type_affinity_score, 0.5);
        approx_eq(candidate_result.total_score, (1.5 / 2.0_f64.ln()) + 0.5);
    }

    #[test]
    fn applies_type_affinity_matrix() {
        let seed = page("seed", "Seed", Some("concept"), &[1], &[]);
        let entity = page("entity", "Entity", Some("entity"), &[1], &[]);
        let query = page("query", "Query", Some("query"), &[1], &[]);

        let results = related(&seed, &[query, entity], 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].slug, "entity");
        approx_eq(results[0].type_affinity_score, 1.2);
        approx_eq(results[0].total_score, 5.2);
        approx_eq(results[1].type_affinity_score, 1.0);
        approx_eq(results[1].total_score, 5.0);
    }

    #[test]
    fn ignores_type_affinity_without_structural_evidence() {
        let seed = page("seed", "Seed", Some("concept"), &[], &[]);
        let unrelated = page("unrelated", "Unrelated", Some("entity"), &[], &[]);

        let results = related(&seed, &[unrelated], 10);

        assert!(
            results.is_empty(),
            "page kinds must refine real graph evidence, not invent a relationship"
        );
    }

    #[test]
    fn sorts_by_score_then_slug_stably() {
        let seed = page("seed", "Seed", Some("entity"), &[1], &[]);
        let alpha = page("alpha", "Alpha", Some("entity"), &[1], &[]);
        let beta = page("beta", "Beta", Some("entity"), &[1], &[]);
        let gamma = page("gamma", "Gamma", Some("query"), &[1], &[]);

        let results = related(&seed, &[beta, gamma, alpha], 3);
        assert_eq!(
            results
                .iter()
                .map(|result| result.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"]
        );
        approx_eq(results[0].total_score, 4.8);
        approx_eq(results[1].total_score, 4.8);
        approx_eq(results[2].total_score, 4.8);
    }

    #[test]
    fn builds_deterministic_document_span_term_and_occurrence_graph() {
        let content = "# Design\n\nRust graph. Rust works.\n";
        let input = DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: "design",
            label: "Design",
            content,
        };

        let graph = build_document_graph(&input).unwrap();

        assert_eq!(graph.document_node_id, "page:design");
        assert_eq!(graph.nodes.len(), 10);
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.node_type == "passage")
                .count(),
            2
        );
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.node_type == "sentence")
                .count(),
            3
        );
        assert!(graph.nodes.iter().any(|node| node.node_id == "term:rust"));
        assert_eq!(graph.edges.len(), 22);
        let document_occurrence = graph
            .edges
            .iter()
            .find(|edge| {
                edge.edge_type == "OCCURS_IN"
                    && edge.from_node_id == "term:rust"
                    && edge.to_node_id == "page:design"
            })
            .unwrap();
        assert_eq!(document_occurrence.frequency, Some(2));
        assert_eq!(
            document_occurrence
                .positions
                .iter()
                .map(|range| &content[range.clone()])
                .collect::<Vec<_>>(),
            vec!["Rust", "Rust"]
        );
        assert_eq!(build_document_graph(&input).unwrap(), graph);
    }

    #[test]
    fn cooccurrence_uses_strong_sentence_and_weak_passage_weights() {
        let input = DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: "weights",
            label: "Weights",
            content: "alpha beta. Gamma.",
        };

        let build = build_cooccurrence(&input).unwrap();
        let contribution = |from: &str, to: &str| {
            build
                .contributions
                .iter()
                .find(|pair| pair.from_term_id == from && pair.to_term_id == to)
                .unwrap()
        };

        let alpha_beta = contribution("term:alpha", "term:beta");
        approx_eq(alpha_beta.sentence_weight, 0.5);
        approx_eq(alpha_beta.passage_weight, 0.0);
        assert_eq!(alpha_beta.witness_count, 1);
        let beta_gamma = contribution("term:beta", "term:gamma");
        approx_eq(beta_gamma.sentence_weight, 0.0);
        approx_eq(beta_gamma.passage_weight, 0.125);
        assert_eq!(beta_gamma.witness_count, 1);
        let alpha_gamma = contribution("term:alpha", "term:gamma");
        approx_eq(alpha_gamma.passage_weight, 0.25 / 3.0);
        assert_eq!(build.truncated_sentence_count, 0);
    }

    #[test]
    fn cooccurrence_ranking_uses_weighted_dice_and_deterministic_top_k() {
        let pair = |from: &str, to: &str, weight: f64, witnesses: usize| TermPairContribution {
            from_term_id: from.to_string(),
            to_term_id: to.to_string(),
            sentence_weight: weight,
            passage_weight: 0.0,
            witness_count: witnesses,
        };
        let contributions = vec![
            pair("term:a", "term:b", 2.0, 2),
            pair("term:b", "term:a", 2.0, 2),
            pair("term:a", "term:c", 1.0, 1),
            pair("term:c", "term:a", 1.0, 1),
        ];

        let ranked = rank_cooccurrence(&contributions, 1);

        assert_eq!(ranked.len(), 3);
        let a = ranked
            .iter()
            .find(|edge| edge.from_term_id == "term:a")
            .unwrap();
        assert_eq!(a.to_term_id, "term:b");
        approx_eq(a.normalized_strength, 0.8);
        approx_eq(a.raw_strength, 2.0);
        assert_eq!(a.witness_count, 2);
        assert_eq!(a.rank, 1);
    }

    #[test]
    fn structured_markdown_does_not_create_natural_language_cooccurrence() {
        for content in [
            "| customer_field | varchar_value |\n| --- | --- |\n| account_status | enabled |",
            "```sql\nSELECT customer_field FROM account_table WHERE account_status = 'enabled';\n```",
            "<table><tr><td>customer_field</td><td>account_status</td></tr></table>",
        ] {
            let built = build_cooccurrence(&DocumentGraphInput {
                document_type: DocumentType::Page,
                identifier: "structured-snapshot",
                label: "Structured snapshot",
                content,
            })
            .unwrap();

            assert!(
                built.contributions.is_empty(),
                "structured passage created prose co-occurrence: {content:?}"
            );
        }
    }

    #[test]
    fn cooccurrence_top_k_never_exceeds_the_outgoing_limit() {
        let mut contributions = Vec::new();
        for index in 0..40 {
            let neighbor = format!("term:n{index:02}");
            for (from, to) in [
                ("term:hub".to_string(), neighbor.clone()),
                (neighbor, "term:hub".to_string()),
            ] {
                contributions.push(TermPairContribution {
                    from_term_id: from,
                    to_term_id: to,
                    sentence_weight: 1.0,
                    passage_weight: 0.0,
                    witness_count: 1,
                });
            }
        }

        let ranked = rank_cooccurrence(&contributions, 32);
        let hub = ranked
            .iter()
            .filter(|edge| edge.from_term_id == "term:hub")
            .collect::<Vec<_>>();

        assert_eq!(hub.len(), 32);
        assert_eq!(hub[0].to_term_id, "term:n00");
        assert_eq!(hub[31].to_term_id, "term:n31");
        assert_eq!(
            hub.iter().map(|edge| edge.rank).collect::<Vec<_>>(),
            (1..=32).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cooccurrence_truncation_does_not_remove_searchable_terms() {
        let content = (0..513)
            .map(|index| format!("t{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let input = DocumentGraphInput {
            document_type: DocumentType::Source,
            identifier: "1",
            label: "Large sentence",
            content: &content,
        };

        let cooccurrence = build_cooccurrence(&input).unwrap();
        let graph = build_document_graph(&input).unwrap();

        assert_eq!(cooccurrence.truncated_sentence_count, 1);
        assert!(
            !cooccurrence
                .contributions
                .iter()
                .any(|pair| { pair.from_term_id == "term:t512" || pair.to_term_id == "term:t512" })
        );
        assert!(graph.nodes.iter().any(|node| node.node_id == "term:t512"));
    }

    #[test]
    fn occurrence_truth_above_the_per_edge_cap_fails_instead_of_truncating() {
        let content = "知".repeat(65_537);
        let error = build_document_graph(&DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: "position-cap",
            label: "Position cap",
            content: &content,
        })
        .unwrap_err();
        assert_eq!(
            error,
            SegmentError::TooManyPositions {
                limit: 65_536,
                actual: 65_537,
            }
        );
    }

    #[test]
    fn graph_digest_and_delta_are_complete_deterministic_and_noop_aware() {
        let old_input = DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: "mutable",
            label: "Mutable",
            content: "alpha.",
        };
        let new_input = DocumentGraphInput {
            content: "beta.",
            ..old_input.clone()
        };
        let old = build_document_graph(&old_input).unwrap();
        let new = build_document_graph(&new_input).unwrap();

        let deltas = diff_document_graph(Some(&old), Some(&new));

        assert!(!canonical_graph_digest(&old).is_empty());
        assert_eq!(canonical_graph_digest(&old), canonical_graph_digest(&old));
        assert_ne!(canonical_graph_digest(&old), canonical_graph_digest(&new));
        assert!(deltas.iter().any(|delta| {
            delta.action == "update"
                && delta.entity_type == "node"
                && delta.entity_id == "page:mutable"
                && delta.before_json.is_some()
                && delta.after_json.is_some()
        }));
        assert!(deltas.iter().any(|delta| {
            delta.action == "remove"
                && delta.entity_id == "term:alpha"
                && delta.before_json.is_some()
                && delta.after_json.is_none()
        }));
        assert!(deltas.iter().any(|delta| {
            delta.action == "add"
                && delta.entity_id == "term:beta"
                && delta.before_json.is_none()
                && delta.after_json.is_some()
        }));
        assert!(diff_document_graph(Some(&new), Some(&new)).is_empty());
    }
}
