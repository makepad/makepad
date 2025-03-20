use crate::makepad_micro_serde::*;

#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiChatPrompt {
    pub contents: Vec<GoogleAiContent>,
}

#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiContent {
    pub role: Option<String>,
    pub parts: Vec<GoogleAiPart>,
} 

#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiPart {
    pub text: String,
} 

#[allow(non_snake_case)]
#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiResponse{
    pub candidates: Vec<GoogleAiCandidate>,
    pub usageMetadata: GoogleAiMetadata,
    pub modelVersion: String,
}

#[allow(non_snake_case)]
#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiCitation {
    pub citationSources: Vec<GoogleAiCitationSource>,
} 

#[allow(non_snake_case)]
#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiCitationSource {
    pub startIndex: usize,
    pub endIndex: usize,
    pub uri: String,
    pub license: String,
} 

#[allow(non_snake_case)]
#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiMetadata {
    pub promptTokenCount: usize,
    pub candidatesTokenCount: usize,
    pub totalTokenCount: usize,
    pub promptTokensDetails: Vec<GoogleAiTokenDetail>
} 

#[allow(non_snake_case)]
#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiTokenDetail {
    modality: String,
    tokenCount: usize
}

#[allow(non_snake_case)]
#[derive(Debug, SerJson, DeJson)]
pub struct GoogleAiCandidate {
    pub content: GoogleAiContent,
    pub finishReason: Option<String>,
    pub index: usize,
    pub citationMetadata: Option<GoogleAiCitation>,
} 
