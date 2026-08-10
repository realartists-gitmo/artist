use artist_kernel::{Provenance, Term, TheoryBuilder, TheoryError};

/// Canonical empirical type names.
pub const QUANTITY: &str = "artist.empirical/Quantity";
pub const VALUE: &str = "artist.empirical/Value";
pub const ESTIMATE: &str = "artist.empirical/Estimate";
pub const SEQUENCE: &str = "artist.empirical/Sequence";
pub const PROBABILITY: &str = "artist.empirical/Probability";
pub const MODEL: &str = "artist.empirical/Model";
pub const DATASET: &str = "artist.empirical/Dataset";
pub const PROPOSITION: &str = "artist.empirical/Proposition";

/// Canonical certified claim constructors.
pub const EXACT: &str = "artist.empirical/Exact";
pub const ENCLOSURE: &str = "artist.empirical/Enclosure";
pub const COVERAGE: &str = "artist.empirical/Coverage";
pub const CONVERGES: &str = "artist.empirical/Converges";
pub const POSTERIOR: &str = "artist.empirical/Posterior";

/// Installs only the formal vocabulary needed to state empirical guarantees. It
/// performs no inference and introduces each constructor as an explicit axiom whose
/// provenance remains visible in dependency reports.
pub fn install_empirical_vocabulary(builder: &mut TheoryBuilder) -> Result<(), TheoryError> {
    let provenance = Provenance {
        source: "artist-empirical canonical vocabulary".to_owned(),
        external_ref: None,
        note: Some(
            "Primitive constructors; semantics are fixed by explicit theory declarations and theorems"
                .into(),
        ),
    };
    for name in [
        QUANTITY,
        VALUE,
        ESTIMATE,
        SEQUENCE,
        PROBABILITY,
        MODEL,
        DATASET,
        PROPOSITION,
    ] {
        builder.axiom(name, Term::universe(0), provenance.clone())?;
    }
    builder.axiom(EXACT, arrows([QUANTITY, VALUE]), provenance.clone())?;
    builder.axiom(
        ENCLOSURE,
        arrows([QUANTITY, VALUE, VALUE]),
        provenance.clone(),
    )?;
    builder.axiom(
        COVERAGE,
        arrows([ESTIMATE, QUANTITY, VALUE, PROBABILITY]),
        provenance.clone(),
    )?;
    builder.axiom(CONVERGES, arrows([SEQUENCE, QUANTITY]), provenance.clone())?;
    builder.axiom(
        POSTERIOR,
        arrows([MODEL, DATASET, PROPOSITION, PROBABILITY]),
        provenance,
    )?;
    Ok(())
}

fn arrows<const N: usize>(domains: [&str; N]) -> Term {
    Term::pi_many(domains.into_iter().map(Term::constant), Term::universe(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_vocabulary_installs_all_claim_constructors() {
        let mut builder = TheoryBuilder::new("empirical-test", "1");
        install_empirical_vocabulary(&mut builder).unwrap();
        let theory = builder.finish();
        for name in [EXACT, ENCLOSURE, COVERAGE, CONVERGES, POSTERIOR] {
            assert!(
                theory
                    .declaration(&artist_kernel::Name::from(name))
                    .is_some()
            );
        }
    }
}
