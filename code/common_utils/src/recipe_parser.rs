use std::collections::HashMap;
use std::error;
use std::fmt;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct RecipeParsingError {
    pub error_desc: String,
}
impl fmt::Display for RecipeParsingError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.error_desc)
    }
}
impl error::Error for RecipeParsingError {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: u64,
    pub issue_id: u64,
    pub url: String,
    pub title: String,
    pub ingredients: Vec<HashMap<String, String>>,
    pub steps: Vec<String>,
    pub image_url: Option<String>,
}

pub struct Task {
    pub id: u64,
    pub issue_id: u64,
    pub document_string: String,
    pub url: String,
}

pub trait RecipeParser {
    fn parse_recipe(task: &Task) -> anyhow::Result<Option<Recipe>>;
}
