// Provider implementations.
//
// `anthropic` handles every upstream that speaks the Anthropic Messages
// API natively (Anthropic, DeepSeek's /anthropic, OpenRouter's Anthropic
// endpoint, etc.). `openai` and `passthrough` arrive in later phases.

pub mod anthropic;
