use artist_cognition::kernel::{
    Certificate, CheckSession, Context, Kernel, KernelRequest, Provenance, SessionStatus, Term,
    TheoryBuilder,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = TheoryBuilder::new("example.resume", "1");
    builder.axiom("P", Term::universe(0), Provenance::new("example premise"))?;
    builder.axiom("p", Term::constant("P"), Provenance::new("example premise"))?;
    let theory = builder.finish();
    let certificate = Certificate::new(
        &theory,
        Context::new(),
        Term::constant("P"),
        Term::constant("p"),
    );

    let mut session = Kernel::start(theory, KernelRequest::Certificate { certificate });
    assert!(matches!(
        session.run_slice(1),
        SessionStatus::Running { .. }
    ));

    let checkpoint = serde_json::to_vec(&session)?;
    let mut resumed: CheckSession = serde_json::from_slice(&checkpoint)?;
    loop {
        match resumed.run_slice(1) {
            SessionStatus::Running { .. } => {}
            SessionStatus::Accepted { steps, result } => {
                println!("accepted after {steps} transitions: {result:?}");
                break;
            }
            SessionStatus::Rejected { error, .. } => return Err(error.into()),
        }
    }
    Ok(())
}
