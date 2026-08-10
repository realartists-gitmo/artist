use artist_cognition::kernel::{
    Certificate, Context, Kernel, KernelRequest, KernelResult, Provenance, Term, TheoryBuilder,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut theory = TheoryBuilder::new("example.naturals", "1");
    theory.axiom(
        "Nat",
        Term::universe(0),
        Provenance::new("example signature"),
    )?;
    theory.axiom(
        "zero",
        Term::constant("Nat"),
        Provenance::new("example signature"),
    )?;
    theory.define(
        "identity",
        Term::pi(Term::constant("Nat"), Term::constant("Nat")),
        Term::lam(Term::constant("Nat"), Term::var(0)),
        Provenance::new("checked definition"),
    )?;
    let theory = theory.finish();

    let proposition = Term::constant("Nat");
    let proof = Term::app(Term::constant("identity"), Term::constant("zero"));
    let certificate = Certificate::new(&theory, Context::new(), proposition, proof);

    let result = Kernel::run_to_completion(
        theory.clone(),
        KernelRequest::Certificate {
            certificate: certificate.clone(),
        },
    )?;
    assert_eq!(result, KernelResult::Certified);

    println!("accepted: {} ; [] ⊢ Nat", theory.id());
    for dependency in certificate.dependencies(&theory).dependencies {
        println!(
            "dependency: {} ({:?}) from {}",
            dependency.name, dependency.kind, dependency.source
        );
    }
    Ok(())
}
