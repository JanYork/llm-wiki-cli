use std::collections::{HashMap, HashSet};

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
    use super::{GraphPage, related};

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
}
