use common_utils::recipe_parser::{Recipe, RecipeParser, Task};

use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;
use lazy_static::lazy_static;
use regex::Regex;
use sxd_document::dom::Document;
use sxd_html::parse_html;
use sxd_xpath::{evaluate_xpath, nodeset::Node, Value};

pub struct ElementareeRecipeParser;

pub fn remove_newlines(mut s: String) -> String {
    s.retain(|ch| ch != '\n' && ch != '\r');
    s
}

pub fn shorten_whitespace(mut s: String) -> String {
    let mut prev = '\0';
    s.retain(|ch| {
        let two_spaces = (ch == ' ') && (prev == ' ');
        prev = ch;
        !two_spaces
    });
    s
}

fn parse_title(document: &Document) -> String {
    let raw_title = evaluate_xpath(&document, "/html/body/div[2]/section/div[2]/h2")
        .expect("XPath evaluation failed");
    let title = raw_title.string();
    println!("raw_title: {}", title);
    let title = title.trim();
    println!("after trim: {}", title);
    let title = remove_newlines(title.to_owned());
    println!("after remove_newlines: {}", title);
    let title = shorten_whitespace(title);
    println!("after shorten_whitespace: {}", title);
    println!("{} '{}'", "title:".red().bold(), title.purple().bold());
    title
}

fn parse_image_link(document: &Document) -> Option<String> {
    let image_value = evaluate_xpath(
        &document,
        "/html/body/div[2]/section/div[3]/div/div[1]/picture/img",
    )
    .expect("XPath evaluation failed");

    let mut image_url: Option<String> = None;

    if let Value::Nodeset(nodes) = image_value {
        for node in nodes.document_order() {
            if let Node::Element(elem) = node {
                for attr in elem.attributes() {
                    if attr.name().local_part() == "src" {
                        println!(
                            "{} {}='{}'",
                            "image:".red().bold(),
                            attr.name().local_part(),
                            attr.value().blue()
                        );
                        image_url = Some(attr.value().to_owned());
                    }
                }
            }
        }
    }

    image_url
}

lazy_static! {
    static ref ingredient_with_quantity_and_braces_regex: Regex =
        Regex::new(r"^\s*(?<name>\b.+\b\s*?)+\s*\(?:(?<amount>[0-9]+)\s*\b(?<unit>.+)\b\)\s*$")
            .unwrap();
}

lazy_static! {
    static ref ingredient_with_quantity_and_minus_regex: Regex =
        Regex::new(r"^\s*(?<name>\b.+\b\s*?)+\s*[-]+(?<amount>[0-9]+)\s*\b(?<unit>.+)\b\s*$")
            .unwrap();
}

lazy_static! {
    static ref ingredient_without_quantity_regex: Regex =
        Regex::new(r"^\s*(\S+\s*?)+\s*$").unwrap();
}

fn insert_with_amounts(
    ingredient_record: &mut HashMap<String, String>,
    captures: &regex::Captures,
) {
    println!(
        "{} '{}'; {} '{}'; {} '{}'",
        "name:".red().bold(),
        &captures["name"],
        "unit:".red().bold(),
        &captures["unit"],
        "amount:".red().bold(),
        &captures["amount"]
    );
    ingredient_record.insert("name".to_owned(), captures["name"].to_owned());
    ingredient_record.insert("unit".to_owned(), captures["unit"].to_owned());
    ingredient_record.insert("amount".to_owned(), captures["amount"].to_owned());
}

fn insert_without_amounts(
    ingredient_record: &mut HashMap<String, String>,
    ingredient_match: &regex::Match,
) {
    println!("{} '{}'", "name:".red().bold(), ingredient_match.as_str());
    ingredient_record.insert("name".to_owned(), ingredient_match.as_str().to_owned());
    ingredient_record.insert("unit".to_owned(), "none".to_owned());
    ingredient_record.insert("amount".to_owned(), "none".to_owned());
}

fn parse_ingredients(document: &Document) -> (usize, Vec<HashMap<String, String>>) {
    let mut parsed_ingredients = Vec::<HashMap<String, String>>::new();
    let mut max_index = 0;
    let mut ingredients_started = false;
    for index in 1..10 {
        let ingredient_xpath = format!(
            "/html/body/div[2]/section/div[3]/div/div[5]/div[1]/div[1]/p[{}]",
            index
        );
        let ingredient_batch = evaluate_xpath(document, ingredient_xpath.as_str())
            .expect("XPath evaluation failed")
            .string();
        let mut ingredient_batch = ingredient_batch.trim().to_owned();
        if !ingredients_started {
            if ingredient_batch.to_lowercase() == "вам понадобится" {
                ingredients_started = true;
                continue;
            }
        }
        if ingredient_batch.to_lowercase() == "на вашей кухне" {
            max_index = index;
            break;
        }
        println!(
            "{}{}{} {}",
            "ingredient_batch[".green().bold(),
            index - 1,
            "]:".green().bold(),
            ingredient_batch
        );
        println!("after remove_newlines: {}", ingredient_batch);
        ingredient_batch = remove_newlines(ingredient_batch);
        println!("after remove_newlines: {}", ingredient_batch);
        ingredient_batch = shorten_whitespace(ingredient_batch);
        println!("after shorten_whitespace: {}", ingredient_batch);
        if let Some(ingredients) = ingredient_batch.splitn(2, ':').nth(1) {
            ingredient_batch = ingredients.to_owned();
        };
        println!(
            "{} {}",
            "ingredient_batch_splitted:".green().bold(),
            ingredient_batch
        );
        let ingredients_list = ingredient_batch
            .split(',')
            .map(|line| line.trim().to_owned())
            .collect::<Vec<String>>();
        println!("{} {:#?}", "ingredients:".green().bold(), ingredients_list);
        for ingredient in ingredients_list.iter() {
            let mut ingredient_record = HashMap::<String, String>::new();
            if let Some(captures) = ingredient_with_quantity_and_braces_regex.captures(ingredient) {
                insert_with_amounts(&mut ingredient_record, &captures);
            } else if let Some(captures) =
                ingredient_with_quantity_and_braces_regex.captures(ingredient)
            {
                insert_with_amounts(&mut ingredient_record, &captures);
            } else if let Some(capture) = ingredient_without_quantity_regex.find(ingredient) {
                insert_without_amounts(&mut ingredient_record, &capture);
            } else {
                eprintln!(
                    "{} can't parse ingredient: '{}'",
                    "Warning:".yellow().bold(),
                    ingredient
                );
                continue;
            }
            parsed_ingredients.push(ingredient_record);
        }
    }
    (max_index, parsed_ingredients)
}

lazy_static! {
    static ref how_to_cook_regex: Regex = Regex::new(r"как готовить").unwrap();
}

struct MDRemover;

impl regex::Replacer for MDRemover {
    fn replace_append(&mut self, caps: &regex::Captures<'_>, dst: &mut String) {
        dst.push_str(&caps["left"]);
        dst.push_str(&caps["inner"]);
        dst.push_str(&caps["right"]);
    }
}

lazy_static! {
    static ref markdown_asterisks_bold: Regex =
        Regex::new(r"(?<left>.*)\*\*(?<inner>\S+)\*\*(?<right>.*)").unwrap();
}

fn parse_steps(document: &Document, start_xpath_index: usize) -> Vec<String> {
    println!("{}", "steps:".red().bold());
    let mut steps = Vec::<String>::new();
    let mut steps_started = false;
    for index in start_xpath_index..100 {
        let step_xpath = format!(
            "/html/body/div[2]/section/div[3]/div/div[5]/div[1]/div[1]/p[{}]",
            index
        );
        let step = evaluate_xpath(&document, step_xpath.as_str())
            .expect("XPath evaluation failed")
            .string();
        let step = step.trim();
        println!("after trim: {}", step);
        let step = remove_newlines(step.to_owned());
        println!("after remove_newlines: {}", step);
        let step = shorten_whitespace(step);
        println!("after shorten_whitespace: {}", step);
        if !steps_started {
            if how_to_cook_regex
                .find(step.to_lowercase().as_str())
                .is_some()
            {
                steps_started = true;
            }
            continue;
        }
        let step_lowercased = step.to_lowercase();
        if step_lowercased.starts_with("если к вам попал овощ не лучшего качества")
            || step_lowercased.starts_with("корректируйте время приготовления")
            || step.trim().is_empty()
        {
            break;
        }

        if step.chars().next().unwrap().is_lowercase() {
            match steps.last_mut() {
                Some(last) => {
                    last.push(' ');
                    last.push_str(step.as_str())
                }
                None => steps.push(step.to_owned()),
            }
            continue;
        }
        let step = markdown_asterisks_bold
            .replace(step.as_str(), MDRemover)
            .into_owned();
        steps.push(step);
    }
    println!("{:#?}", steps);
    steps
}

impl RecipeParser for ElementareeRecipeParser {
    fn parse_recipe(task: &Task) -> Result<Option<Recipe>> {
        println!("{} {}", "start_parsing:".purple().bold(), task.url.blue());
        let package = parse_html(task.document_string.as_str());
        let document = package.as_document();
        let title = parse_title(&document);
        if title.is_empty() {
            eprintln!("{} empty title", "Warning:".yellow().bold());
            println!("{} {}", "end_parsing:".purple().bold(), task.url.blue());
            return Ok(None);
        }
        let image_url = parse_image_link(&document);
        let (last_ingredient_xpath_index, ingredients) = parse_ingredients(&document);
        if ingredients.is_empty() {
            eprintln!("{} empty ingredients", "Warning:".yellow().bold());
            println!("{} {}", "end_parsing:".purple().bold(), task.url.blue());
            return Ok(None);
        }
        let steps = parse_steps(&document, last_ingredient_xpath_index);
        if steps.is_empty() {
            eprintln!("{} empty steps", "Warning:".yellow().bold());
            println!("{} {}", "end_parsing:".purple().bold(), task.url.blue());
            return Ok(None);
        }
        println!("{} {}", "end_parsing:".purple().bold(), task.url.blue());
        println!();
        println!();

        Ok(Some(Recipe {
            id: task.id,
            issue_id: task.issue_id,
            url: task.url.clone(),
            title,
            ingredients,
            steps,
            image_url,
        }))
    }
}
