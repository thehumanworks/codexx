pub enum Event<'a, S, T> {
    SessionStarted(&'a S),
    TurnStarted(&'a T),
    TurnFinished { turn: &'a T, outcome: String }, // Make outcome better?!
    SessionStopping(&'a S),
}
