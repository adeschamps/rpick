/* Copyright © 2019-2021 Randy Barlow
This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, version 3 of the License.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <http://www.gnu.org/licenses/>.*/
//! # rpick
//!
//! ```rpick``` helps pick items from a list of choices, using various algorithms.
//!
//! # Example
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use rand::SeedableRng;
//!
//! /// You need to define an interface. rpick will use this interface to interact with you during
//! /// picks.
//! struct Interface {};
//!
//! impl rpick::ui::UI for Interface {
//!     fn call_display_table(&self) -> bool { false }
//!
//!     fn display_table(&self, table: &rpick::ui::Table) {}
//!
//!     fn info(&self, message: &str) { println!("{}", message); }
//!
//!     fn prompt_choice(&self, choice: &str) -> bool {
//!         println!("{}", choice);
//!         true
//!     }
//! }
//!
//! let ui = Interface{};
//! let mut engine = rpick::Engine::new(&ui);
//! // For the sake of this example, let's override the PRNG with a seeded PRNG so the assertion
//! // works as expected at the end. You most likely do not want to do this in practice as it takes
//! // the randomness out of the system.
//! engine.set_rng(rand::rngs::SmallRng::seed_from_u64(37));
//! let choices = vec![String::from("this"), String::from("that"),
//!                    String::from("the other")];
//! let category = rpick::ConfigCategory::Even{choices: choices};
//! let mut config = BTreeMap::new();
//! config.insert("things".to_string(), category);
//!
//! let choice = engine.pick(&mut config, "things".to_string()).expect("unexpected");
//!
//! // 32-bit architectures have different PRNG results than 64-bit architectures, so we will
//! // only run this assertion on 64-bit systems.
//! #[cfg(target_pointer_width = "64")]
//! assert_eq!(choice, "that");
//! ```
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::{error, fmt};

use rand::seq::SliceRandom;
use rand::Rng;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use statrs::distribution::Univariate;

use ui::{Cell, Row, Table};

pub mod ui;

/// The rpick Engine object allows you to write your own rpick interface.
///
/// # Attributes
///
/// * `ui` - This is a struct that implements the [`ui::UI`] trait.
/// * `rng` - This must be a random number generator that implements the [`rand::RngCore`]
///           trait.
pub struct Engine<'ui, U> {
    ui: &'ui U,
    rng: Box<dyn rand::RngCore>,
}

impl<'a, 'ui, U> Engine<'ui, U>
where
    U: ui::UI,
{
    /// Instantiate an Engine.
    ///
    /// # Arguments
    ///
    /// * `ui` - This is a struct that implements the [`ui::UI`] trait. It is how rpick will
    ///     interact with the caller.
    pub fn new(ui: &'ui U) -> Engine<U> {
        let rng = rand::thread_rng();

        Engine {
            ui,
            rng: Box::new(rng),
        }
    }

    /// Pick an item from the [`ConfigCategory`] referenced by the given `category`.
    ///
    /// # Arguments
    ///
    /// * `config` - A mapping of category names to [`ConfigCategory`] objects, which contain the
    ///              parameters which should be used for the pick.
    /// * `category` - The category you wish to choose from.
    ///
    /// # Returns
    ///
    /// This will return the chosen item.
    pub fn pick(
        &mut self,
        config: &mut BTreeMap<String, ConfigCategory>,
        category: String,
    ) -> Result<String, Box<dyn error::Error>> {
        let config_category = config.get_mut(&category[..]);
        match config_category {
            Some(category) => match category {
                ConfigCategory::Even { choices } => Ok(self.pick_even(choices)),
                ConfigCategory::Gaussian {
                    choices,
                    stddev_scaling_factor,
                } => Ok(self.pick_gaussian(choices, *stddev_scaling_factor)),
                ConfigCategory::Inventory { choices } => Ok(self.pick_inventory(choices)),
                ConfigCategory::Lottery { choices } => Ok(self.pick_lottery(choices)),
                ConfigCategory::LRU { choices } => Ok(self.pick_lru(choices)),
                ConfigCategory::Weighted { choices } => Ok(self.pick_weighted(choices)),
            },
            None => Err(Box::new(ValueError::new(format!(
                "Category {} not found in config.",
                category
            )))),
        }
    }

    /// Use the given random number generator rather than the default.
    pub fn set_rng<R: 'static + Rng>(&mut self, rng: R) {
        self.rng = Box::new(rng);
    }

    /// Express disapproval to the user.
    fn express_disapproval(&mut self) {
        self.ui.info("🤨");
    }

    /// Prompt the user for consent for the given choice, returning a bool true if they accept the
    /// choice, or false if they do not.
    fn get_consent(&mut self, choice: &str) -> bool {
        self.ui.prompt_choice(choice)
    }

    /// Use an even distribution random model to pick from the given choices.
    fn pick_even(&mut self, choices: &[String]) -> String {
        let initialize_candidates = || {
            choices
                .iter()
                .enumerate()
                .map(|x| ((x.0, x.1), 1))
                .collect::<Vec<_>>()
        };

        let index = self.pick_weighted_common(&initialize_candidates);

        choices[index].clone()
    }

    /// Run the gaussian model for the given choices and standard deviation scaling factor. When the
    /// user accepts a choice, move that choice to end of the choices Vector and return.
    fn pick_gaussian(&mut self, choices: &mut Vec<String>, stddev_scaling_factor: f64) -> String {
        let mut candidates = choices.clone();
        let mut index;

        loop {
            let stddev = (candidates.len() as f64) / stddev_scaling_factor;
            let normal = Normal::new(0.0, stddev).unwrap();
            index = normal.sample(&mut self.rng).abs() as usize;

            if let Some(value) = candidates.get(index) {
                if self.ui.call_display_table() {
                    self.display_gaussian_chance_table(index, &candidates, stddev);
                }

                if self.get_consent(&value[..]) {
                    index = choices.iter().position(|x| x == value).unwrap();
                    break;
                } else if candidates.len() > 1 {
                    index = candidates.iter().position(|x| x == value).unwrap();
                    candidates.remove(index);
                } else {
                    self.express_disapproval();
                    candidates = choices.clone();
                }
            }
        }

        let value = choices.remove(index);
        choices.push(value.clone());
        value
    }

    /// Run the inventory model for the given choices.
    fn pick_inventory(&mut self, choices: &mut Vec<InventoryChoice>) -> String {
        let initialize_candidates = || {
            choices
                .iter()
                .enumerate()
                .filter(|x| x.1.tickets > 0)
                .map(|x| ((x.0, &x.1.name), x.1.tickets))
                .collect::<Vec<_>>()
        };

        let index = self.pick_weighted_common(&initialize_candidates);

        choices[index].tickets -= 1;
        choices[index].name.clone()
    }

    /// Run the LRU model for the given choices. When the user accepts a choice, move that choice to
    /// the end of the choices Vector and return.
    fn pick_lru(&mut self, choices: &mut Vec<String>) -> String {
        for (index, choice) in choices.iter().enumerate() {
            if self.ui.call_display_table() {
                self.display_lru_table(index, &choices);
            }

            if self.get_consent(&choice[..]) {
                let chosen = choices.remove(index);
                choices.push(chosen.clone());
                return chosen;
            }
        }
        self.express_disapproval();
        // If we've gotten here, the user hasn't made a choice. So… let's do it again!
        self.pick_lru(choices)
    }

    /// Run the lottery model for the given choices.
    fn pick_lottery(&mut self, choices: &mut Vec<LotteryChoice>) -> String {
        let initialize_candidates = || {
            choices
                .iter()
                .enumerate()
                .filter(|x| x.1.tickets > 0)
                .map(|x| ((x.0, &x.1.name), x.1.tickets))
                .collect::<Vec<_>>()
        };

        let index = self.pick_weighted_common(&initialize_candidates);

        for choice in choices.iter_mut() {
            choice.tickets += choice.weight;
        }
        choices[index].tickets = 0;
        choices[index].name.clone()
    }

    /// Run the weighted model for the given choices.
    fn pick_weighted(&mut self, choices: &[WeightedChoice]) -> String {
        let initialize_candidates = || {
            choices
                .iter()
                .enumerate()
                .map(|x| ((x.0, &x.1.name), x.1.weight))
                .collect::<Vec<_>>()
        };

        let index = self.pick_weighted_common(&initialize_candidates);

        choices[index].name.clone()
    }

    /// A common weighted choice algorithm used as the core of many models.
    ///
    /// The initialize_candidates() function should return a Vector of 2-tuples. The first element
    /// of the 2-tuple is also a 2-tuple, specifying the original index of the choice and the human
    /// readable name of the choice. The second element of the outer 2-tuple should express the
    /// weight of that choice. For example, if the first choice is "ice cream" and has a weight of
    /// 5, the data structure would look like this: ((0, "ice cream"), 5)
    fn pick_weighted_common(
        &mut self,
        initialize_candidates: &dyn Fn() -> Vec<((usize, &'a String), u64)>,
    ) -> usize {
        let mut candidates = initialize_candidates();

        loop {
            let (index, choice) = candidates
                .choose_weighted(&mut self.rng, |item| item.1)
                .unwrap()
                .0;

            if self.ui.call_display_table() {
                self.display_weighted_chance_table(index, &candidates);
            }

            if self.get_consent(&choice[..]) {
                break index;
            } else if candidates.len() > 1 {
                candidates.remove(candidates.iter().position(|x| (x.0).1 == choice).unwrap());
            } else {
                self.express_disapproval();
                candidates = initialize_candidates();
            }
        }
    }

    /// Print a table to self.output showing the candidates, sorted by chance of being chosen.
    ///
    /// # Arguments
    ///
    /// `index` - The index of the candidate that was chosen. This is used to turn the chosen
    ///     candidate yellow in the table.
    /// `candidates` - A list of the candidates.
    fn display_gaussian_chance_table(&mut self, index: usize, candidates: &[String], stddev: f64) {
        // Let's make a copy of the candidate list so that we can sort it for the table
        // without sorting the real candidate list.
        let candidates = candidates.to_owned();

        let header: Vec<Cell> = vec!["Name".into(), "Chance".into()];
        let mut rows = vec![];
        let distribution = statrs::distribution::Normal::new(0.0, stddev).unwrap();
        let mut total_chance = 0.0;
        for (i, candidate) in candidates.iter().enumerate() {
            // We multiply by 200 here: 100 is for expressing percents to humans, and the factor
            // of 2 is to account for the abs() we use in pick_gaussian(), which causes us to
            // reflect the distribution around the x-axis (thus the chance is this slice of the CDF
            // on both sides of the x-axis, which is the same chance as twice this singular slice).
            let chance: f64 =
                (distribution.cdf((i as f64) + 1.0) - distribution.cdf(i as f64)) * 200.;
            total_chance += chance;
            let mut cells: Vec<Cell> = vec![];
            let chosen = i == index;
            cells.push(Cell::from(candidate.as_ref()));
            cells.push(chance.into());
            let row = Row { cells, chosen };
            rows.push(row);
        }
        let footer: Vec<Cell> = vec!["Total".into(), total_chance.into()];

        self.ui.display_table(&Table {
            header,
            rows,
            footer,
        });
    }

    /// Print a table to self.output showing the candidates, sorted by chance of being chosen.
    ///
    /// # Arguments
    ///
    /// `index` - The index of the candidate that was chosen. This is used to turn the chosen
    ///     candidate yellow in the table.
    /// `candidates` - A list of the candidates.
    fn display_lru_table(&mut self, index: usize, candidates: &[String]) {
        // Filter out candidates that have already been rejected by the user.
        let candidates = candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| i >= &index)
            .map(|x| x.1)
            .collect::<Vec<_>>();

        let header: Vec<Cell> = vec!["Name".into()];
        let mut rows = vec![];
        for (i, candidate) in candidates.iter().rev().enumerate() {
            let mut cells: Vec<Cell> = vec![];
            let chosen = i == candidates.len() - 1;
            cells.push(Cell::from(candidate.as_ref()));
            rows.push(Row { cells, chosen });
        }
        let footer = vec![];

        self.ui.display_table(&Table {
            header,
            rows,
            footer,
        });
    }

    /// Print a table to self.output showing the candidates, sorted by chance of being chosen.
    ///
    /// # Arguments
    ///
    /// `index` - The index of the candidate that was chosen. This is used to turn the chosen
    ///     candidate yellow in the table.
    /// `candidates` - A list of the candidates.
    fn display_weighted_chance_table(
        &mut self,
        index: usize,
        candidates: &[((usize, &'a String), u64)],
    ) {
        // Let's make a copy of the candidate list so that we can sort it for the table
        // without sorting the real candidate list.
        let mut candidates = candidates.to_owned();
        candidates.sort_by_key(|c| c.1);

        let total: u64 = candidates.iter().map(|x| x.1).sum();

        let mut rows = vec![];
        let header: Vec<Cell> = vec!["Name".into(), "Weight".into(), "Chance".into()];
        for candidate in candidates.iter() {
            let chance: f64 = (candidate.1 as f64) / (total as f64) * 100.;
            let mut cells: Vec<Cell> = vec![];
            let chosen = (candidate.0).0 == index;
            cells.push(Cell::from((candidate.0).1.as_ref()));
            cells.push(candidate.1.into());
            cells.push(chance.into());
            rows.push(Row { cells, chosen });
        }
        let footer: Vec<Cell> = vec!["Total".into(), total.into(), 100.00.into()];

        self.ui.display_table(&Table {
            header,
            rows,
            footer,
        });
    }
}

/// Returned in the event that an invalid parameter was used in the API.
#[derive(Debug)]
struct ValueError {
    message: String,
}

impl ValueError {
    /// Construct a new ValueError.
    ///
    /// # Arguments
    ///
    /// * `message` - The error message to accompany the ValueError.
    fn new(message: String) -> ValueError {
        ValueError { message }
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl error::Error for ValueError {}

/// Return the user's config as a BTreeMap.
///
/// # Arguments
///
/// * `config_file_path` - A filesystem path to a YAML file that should be read.
///
/// # Returns
///
/// Returns a mapping of YAML to [`ConfigCategory`]'s, or an Error.
pub fn read_config(
    config_file_path: &str,
) -> Result<BTreeMap<String, ConfigCategory>, Box<dyn error::Error>> {
    let f = File::open(&config_file_path)?;
    let reader = BufReader::new(f);

    let config: BTreeMap<String, ConfigCategory> = serde_yaml::from_reader(reader)?;
    Ok(config)
}

/// Save the data from the given BTreeMap to the user's config file.
///
/// # Arguments
///
/// * `config_file_path` - A filesystem path that the config should be written to.
/// * `config` - The config that should be serialized as YAML.
pub fn write_config(
    config_file_path: &str,
    config: BTreeMap<String, ConfigCategory>,
) -> Result<(), Box<dyn error::Error>> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&config_file_path)?;
    let yaml = serde_yaml::to_string(&config).unwrap();

    f.write_all(&yaml.into_bytes())?;
    Ok(())
}

/// A category of items that can be chosen from.
///
/// Each variant of this Enum maps to one of the supported algorithms.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "model")]
pub enum ConfigCategory {
    /// The Even variant picks from its choices with even distribution.
    ///
    /// # Attributes
    ///
    /// * `choices` - The list of choices to pick from.
    Even { choices: Vec<String> },
    /// The Gaussian variant uses a
    /// [Gaussian distribution](https://en.wikipedia.org/wiki/Normal_distribution) to prefer choices
    /// near the beginning of the list of choices over those at the end. Once a choice has been
    /// accepted, it is moved to the end of the list.
    ///
    /// # Attributes
    ///
    /// * `stddev_scaling_factor` - This is used to derive the standard deviation; the standard
    ///   deviation is the length of the list of choices, divided by this scaling factor.
    /// * `choices` - The list of choices to pick from.
    Gaussian {
        #[serde(default = "default_stddev_scaling_factor")]
        stddev_scaling_factor: f64,
        choices: Vec<String>,
    },
    /// The Inventory variant uses a weighted distribution to pick items, with each items chances
    /// being tied to how many tickets it has. When a choice is accepted, that choice's ticket
    /// count is reduced by 1.
    ///
    /// # Attributes
    ///
    /// * `choices` - The list of choices to pick from.
    Inventory { choices: Vec<InventoryChoice> },
    /// The LRU variant picks the Least Recently Used item from the list of choices. The least
    /// recently used choice is found at the beginning of the list. Once a choice has been
    /// accepted, it is moved to the end of the list.
    ///
    /// # Attributes
    ///
    /// * `choices` - The list of choices to pick from.
    #[serde(rename = "lru")]
    LRU { choices: Vec<String> },
    /// The Lottery variant uses a weighted distribution to pick items, with each items chances
    /// being tied to how many tickets it has. When a choice is accepted, that choice's ticket
    /// count is set to 0, and every choice not chosen receives its weight in additional tickets.
    ///
    /// # Attributes
    ///
    /// * `choices` - The list of choices to pick from.
    Lottery { choices: Vec<LotteryChoice> },
    /// The Weighted variant is a simple weighted distribution.
    ///
    /// # Attributes
    ///
    /// * `choices` - The list of choices to pick from.
    Weighted { choices: Vec<WeightedChoice> },
}

/// Represents an individual choice for the inventory model.
///
/// # Attributes
///
/// * `name` - The name of the choice.
/// * `tickets` - The current number of tickets the choice has.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct InventoryChoice {
    pub name: String,
    #[serde(default = "default_weight")]
    pub tickets: u64,
}

/// Represents an individual choice for the lottery model.
///
/// # Attributes
///
/// * `name` - The name of the choice.
/// * `tickets` - The current number of tickets the choice has.
/// * `weight` - The number of tickets that will be added to `tickets` each time this choice is not
///   picked.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct LotteryChoice {
    pub name: String,
    #[serde(default = "default_weight")]
    pub tickets: u64,
    #[serde(default = "default_weight")]
    pub weight: u64,
}

/// Represents an individual choice for the weighted model.
///
/// # Attributes
///
/// * `name` - The name of the choice
/// * `weight` - How much chance this choice has of being chosen, relative to the other choices.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct WeightedChoice {
    name: String,
    #[serde(default = "default_weight")]
    weight: u64,
}

/// Define the default for the stddev_scaling_factor setting as 3.0.
fn default_stddev_scaling_factor() -> f64 {
    3.0
}

/// Define the default for the weight setting as 1.
fn default_weight() -> u64 {
    1
}

#[cfg(test)]
mod tests {
    use approx::abs_diff_eq;
    use mockall::predicate;
    use rand::SeedableRng;

    use super::*;

    struct FakeRng(u32);

    /// This allows our tests to have predictable results, and to have the same predictable results
    /// on both 32-bit and 64-bit architectures. This is used for all tests except for the Gaussian
    /// tests, since those do behave differently between 32-bit and 64-bit systems when using this
    /// rng.
    impl rand::RngCore for FakeRng {
        fn next_u32(&mut self) -> u32 {
            self.0 += 1;
            self.0 - 1
        }

        fn next_u64(&mut self) -> u64 {
            self.next_u32() as u64
        }

        fn fill_bytes(&mut self, dest: &mut [u8]) {
            let mut left = dest;
            while left.len() >= 4 {
                let (l, r) = { left }.split_at_mut(4);
                left = r;
                let chunk: [u8; 4] = self.next_u32().to_le_bytes();
                l.copy_from_slice(&chunk);
            }
            let n = left.len();
            if n > 0 {
                let chunk: [u8; 4] = self.next_u32().to_le_bytes();
                left.copy_from_slice(&chunk[..n]);
            }
        }

        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
            self.fill_bytes(dest);
            Ok(())
        }
    }

    #[test]
    fn test_defaults() {
        assert!((default_stddev_scaling_factor() - 3.0).abs() < 0.000_001);
        assert_eq!(default_weight(), 1);
    }

    #[test]
    fn test_get_consent() {
        let mut ui = ui::MockUI::new();
        ui.expect_prompt_choice()
            .with(predicate::in_iter(vec![
                "you want this",
                "you don't want this",
            ]))
            .times(2)
            .returning(|x| !x.contains("don't"));
        let mut engine = Engine::new(&ui);

        assert_eq!(engine.get_consent("you want this"), true);
        assert_eq!(engine.get_consent("you don't want this"), false);
    }

    #[test]
    fn test_pick() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(2).returning(|| false);
        ui.expect_prompt_choice()
            .with(predicate::in_iter(vec!["that", "this"]))
            .times(2)
            .returning(|c| c == "that");
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];
        let category = ConfigCategory::Even { choices };
        let mut config = BTreeMap::new();
        config.insert("things".to_string(), category);

        let choice = engine
            .pick(&mut config, "things".to_string())
            .expect("unexpected");

        assert_eq!(choice, "that");
    }

    #[test]
    fn test_pick_nonexistant_category() {
        let ui = ui::MockUI::new();
        let mut engine = Engine::new(&ui);
        let choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];
        let category = ConfigCategory::Even { choices };
        let mut config = BTreeMap::new();
        config.insert("things".to_string(), category);

        match engine.pick(&mut config, "does not exist".to_string()) {
            Ok(_) => {
                panic!("The non-existant category should have returned an error.");
            }
            Err(error) => {
                assert_eq!(
                    format!("{}", error),
                    "Category does not exist not found in config."
                );
            }
        }
    }

    #[test]
    fn test_pick_even() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| false);
        ui.expect_prompt_choice()
            .with(predicate::eq("this"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];

        let result = engine.pick_even(&choices);

        assert_eq!(result, "this");
    }

    // Unfortunately, the FakeRng we wrote above causes the Gaussian distribution to often
    // pick outside of the distribution for 32-bit values on 64-bit systems. Since it is a
    // u32, this means that the user saying no here will make the implementation loop forever
    // until it hits MAXINT on 64-bit systems. If we made the FakeRng be a 64 bit value, then
    // the test results on 32-bit systems would overflow. Ideally we'd have a better way than
    // the below to get consistent test results between 32-bit and 64-bit systems, but for now
    // we'll just skip this test on 32-bit systems.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn test_pick_gaussian() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| false);
        ui.expect_prompt_choice()
            .with(predicate::eq("that"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(rand::rngs::SmallRng::seed_from_u64(555));
        let mut choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];

        let result = engine.pick_gaussian(&mut choices, 3.0);

        assert_eq!(result, "that");
        assert_eq!(
            choices,
            vec![
                String::from("this"),
                String::from("the other"),
                String::from("that")
            ]
        );
    }

    // Unfortunately, the FakeRng we wrote above causes the Gaussian distribution to often
    // pick outside of the distribution for 32-bit values on 64-bit systems. Since it is a
    // u32, this means that the user saying no here will make the implementation loop forever
    // until it hits MAXINT on 64-bit systems. If we made the FakeRng be a 64 bit value, then
    // the test results on 32-bit systems would overflow. Ideally we'd have a better way than
    // the below to get consistent test results between 32-bit and 64-bit systems, but for now
    // we'll just skip this test on 32-bit systems.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn test_pick_gaussian_verbose() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| true);
        ui.expect_display_table()
            .withf(|t| {
                println!("{:?}", t);
                let expected_table = ui::Table {
                    footer: vec![ui::Cell::Text("Total"), ui::Cell::Float(99.73)],
                    header: vec![ui::Cell::Text("Name"), ui::Cell::Text("Chance")],
                    rows: vec![
                        ui::Row {
                            cells: vec![ui::Cell::Text("this"), ui::Cell::Float(68.269)],
                            chosen: false,
                        },
                        ui::Row {
                            cells: vec![ui::Cell::Text("that"), ui::Cell::Float(27.181)],
                            chosen: true,
                        },
                        ui::Row {
                            cells: vec![ui::Cell::Text("the other"), ui::Cell::Float(4.280)],
                            chosen: false,
                        },
                    ],
                };
                tables_equal(t, &expected_table)
            })
            .times(1)
            .returning(|_| ());
        ui.expect_prompt_choice()
            .with(predicate::eq("that"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(rand::rngs::SmallRng::seed_from_u64(555));
        let mut choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];

        let result = engine.pick_gaussian(&mut choices, 3.0);

        assert_eq!(result, "that");
        assert_eq!(
            choices,
            vec![
                String::from("this"),
                String::from("the other"),
                String::from("that")
            ]
        );
    }

    #[test]
    fn test_pick_inventory() {
        let mut ui = ui::MockUI::new();
        let mut counter = 0;
        ui.expect_call_display_table().times(4).returning(|| false);
        ui.expect_info()
            .times(1)
            .with(predicate::eq("🤨"))
            .returning(|_| ());
        ui.expect_prompt_choice()
            .times(4)
            .with(predicate::in_iter(vec!["that", "the other"]))
            .returning(move |_| {
                if counter == 3 {
                    true
                } else {
                    counter += 1;
                    false
                }
            });
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let mut choices = vec![
            InventoryChoice {
                name: "this".to_string(),
                tickets: 0,
            },
            InventoryChoice {
                name: "that".to_string(),
                tickets: 2,
            },
            InventoryChoice {
                name: "the other".to_string(),
                tickets: 3,
            },
        ];

        let result = engine.pick_inventory(&mut choices);

        assert_eq!(result, "the other");
        assert_eq!(
            choices,
            vec![
                InventoryChoice {
                    name: "this".to_string(),
                    tickets: 0
                },
                InventoryChoice {
                    name: "that".to_string(),
                    tickets: 2
                },
                InventoryChoice {
                    name: "the other".to_string(),
                    tickets: 2
                }
            ]
        );
    }

    #[test]
    fn test_pick_inventory_verbose() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| true);
        ui.expect_display_table()
            .withf(|t| {
                let expected_table = ui::Table {
                    footer: vec![
                        ui::Cell::Text("Total"),
                        ui::Cell::Unsigned(5),
                        ui::Cell::Float(100.0),
                    ],
                    header: vec![
                        ui::Cell::Text("Name"),
                        ui::Cell::Text("Weight"),
                        ui::Cell::Text("Chance"),
                    ],
                    rows: vec![
                        ui::Row {
                            cells: vec![
                                ui::Cell::Text("that"),
                                ui::Cell::Unsigned(2),
                                ui::Cell::Float(40.0),
                            ],
                            chosen: true,
                        },
                        ui::Row {
                            cells: vec![
                                ui::Cell::Text("the other"),
                                ui::Cell::Unsigned(3),
                                ui::Cell::Float(60.0),
                            ],
                            chosen: false,
                        },
                    ],
                };
                tables_equal(t, &expected_table)
            })
            .times(1)
            .returning(|_| ());
        ui.expect_prompt_choice()
            .with(predicate::eq("that"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let mut choices = vec![
            InventoryChoice {
                name: "this".to_string(),
                tickets: 0,
            },
            InventoryChoice {
                name: "that".to_string(),
                tickets: 2,
            },
            InventoryChoice {
                name: "the other".to_string(),
                tickets: 3,
            },
        ];

        let result = engine.pick_inventory(&mut choices);

        assert_eq!(result, "that");
        assert_eq!(
            choices,
            vec![
                InventoryChoice {
                    name: "this".to_string(),
                    tickets: 0
                },
                InventoryChoice {
                    name: "that".to_string(),
                    tickets: 1
                },
                InventoryChoice {
                    name: "the other".to_string(),
                    tickets: 3
                }
            ]
        );
    }

    #[test]
    fn test_pick_lru() {
        // The user says no to the first one and yes to the second.
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(2).returning(|| false);
        ui.expect_prompt_choice()
            .with(predicate::in_iter(vec!["this", "that"]))
            .times(2)
            .returning(|option| option == "that");
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let mut choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];

        let result = engine.pick_lru(&mut choices);

        assert_eq!(result, "that");
        assert_eq!(
            choices,
            vec![
                String::from("this"),
                String::from("the other"),
                String::from("that")
            ]
        );
    }

    #[test]
    /// Test pick_lru() with the verbose flag set
    fn test_pick_lru_verbose() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| true);
        ui.expect_display_table()
            .withf(|t| {
                let expected_table = ui::Table {
                    footer: vec![],
                    header: vec![ui::Cell::Text("Name")],
                    rows: vec![
                        ui::Row {
                            cells: vec![ui::Cell::Text("the other")],
                            chosen: false,
                        },
                        ui::Row {
                            cells: vec![ui::Cell::Text("that")],
                            chosen: false,
                        },
                        ui::Row {
                            cells: vec![ui::Cell::Text("this")],
                            chosen: true,
                        },
                    ],
                };
                *t == expected_table
            })
            .times(1)
            .returning(|_| ());
        ui.expect_prompt_choice()
            .with(predicate::eq("this"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let mut choices = vec![
            String::from("this"),
            String::from("that"),
            String::from("the other"),
        ];

        let result = engine.pick_lru(&mut choices);

        assert_eq!(result, "this");
        assert_eq!(
            choices,
            vec![
                String::from("that"),
                String::from("the other"),
                String::from("this")
            ]
        );
    }

    #[test]
    fn test_pick_lottery() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| false);
        ui.expect_prompt_choice()
            .with(predicate::eq("this"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let mut choices = vec![
            LotteryChoice {
                name: "this".to_string(),
                tickets: 1,
                weight: 1,
            },
            LotteryChoice {
                name: "that".to_string(),
                tickets: 2,
                weight: 4,
            },
            LotteryChoice {
                name: "the other".to_string(),
                tickets: 3,
                weight: 9,
            },
        ];

        let result = engine.pick_lottery(&mut choices);

        assert_eq!(result, "this");
        assert_eq!(
            choices,
            vec![
                LotteryChoice {
                    name: "this".to_string(),
                    tickets: 0,
                    weight: 1
                },
                LotteryChoice {
                    name: "that".to_string(),
                    tickets: 6,
                    weight: 4
                },
                LotteryChoice {
                    name: "the other".to_string(),
                    tickets: 12,
                    weight: 9
                }
            ]
        );
    }

    /// If the user says no to all the choices, rpick should print out an emoji and start over.
    /// There was previously a bug where the pick would loop forever if one of the options had 0
    /// chance of being picked.
    #[test]
    fn test_pick_lottery_no_to_all_one_no_chance() {
        let mut ui = ui::MockUI::new();
        let mut counter = 0;
        ui.expect_call_display_table().times(4).returning(|| false);
        ui.expect_info()
            .times(1)
            .with(predicate::eq("🤨"))
            .returning(|_| ());
        ui.expect_prompt_choice()
            .times(4)
            .with(predicate::in_iter(vec!["that", "the other"]))
            .returning(move |_| {
                if counter == 3 {
                    true
                } else {
                    counter += 1;
                    false
                }
            });
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let mut choices = vec![
            LotteryChoice {
                name: "this".to_string(),
                tickets: 0,
                weight: 1,
            },
            LotteryChoice {
                name: "that".to_string(),
                tickets: 2,
                weight: 4,
            },
            LotteryChoice {
                name: "the other".to_string(),
                tickets: 3,
                weight: 9,
            },
        ];

        let result = engine.pick_lottery(&mut choices);

        assert_eq!(result, "the other");
        assert_eq!(
            choices,
            vec![
                LotteryChoice {
                    name: "this".to_string(),
                    tickets: 1,
                    weight: 1
                },
                LotteryChoice {
                    name: "that".to_string(),
                    tickets: 6,
                    weight: 4
                },
                LotteryChoice {
                    name: "the other".to_string(),
                    tickets: 0,
                    weight: 9
                }
            ]
        );
    }

    #[test]
    fn test_pick_weighted() {
        let mut ui = ui::MockUI::new();
        ui.expect_call_display_table().times(1).returning(|| false);
        ui.expect_prompt_choice()
            .with(predicate::eq("this"))
            .times(1)
            .returning(|_| true);
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let choices = vec![
            WeightedChoice {
                name: "this".to_string(),
                weight: 1,
            },
            WeightedChoice {
                name: "that".to_string(),
                weight: 4,
            },
            WeightedChoice {
                name: "the other".to_string(),
                weight: 9,
            },
        ];

        let result = engine.pick_weighted(&choices);

        assert_eq!(result, "this");
    }

    /// There was a bug wherein saying no to all weighted options crashed rpick rather than
    /// expressing disapproval.
    #[test]
    fn test_pick_weighted_no_to_all() {
        let mut ui = ui::MockUI::new();
        let mut counter = 0;
        ui.expect_call_display_table().times(4).returning(|| false);
        ui.expect_info()
            .times(1)
            .with(predicate::eq("🤨"))
            .returning(|_| ());
        ui.expect_prompt_choice()
            .times(4)
            .with(predicate::in_iter(vec!["this", "that", "the other"]))
            .returning(move |_| {
                if counter == 3 {
                    true
                } else {
                    counter += 1;
                    false
                }
            });
        let mut engine = Engine::new(&ui);
        engine.set_rng(FakeRng(0));
        let choices = vec![
            WeightedChoice {
                name: "this".to_string(),
                weight: 1,
            },
            WeightedChoice {
                name: "that".to_string(),
                weight: 4,
            },
            WeightedChoice {
                name: "the other".to_string(),
                weight: 9,
            },
        ];

        let result = engine.pick_weighted(&choices);

        assert_eq!(result, "this");
    }

    fn tables_equal(a: &ui::Table, b: &ui::Table) -> bool {
        if !vec_of_cells_equal(&a.footer, &b.footer) {
            println!("Footers not equal: {:?} != {:?}", a.footer, b.footer);
            return false;
        }
        if !vec_of_cells_equal(&a.header, &b.header) {
            println!("Headers not equal: {:?} != {:?}", a.header, b.header);
            return false;
        }
        if !vec_of_rows_equal(&a.rows, &b.rows) {
            println!("Rows not equal: {:?} != {:?}", a.rows, b.rows);
            return false;
        }
        true
    }

    fn vec_of_cells_equal(a: &[Cell], b: &[Cell]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        for (i, cell) in a.iter().enumerate() {
            if let ui::Cell::Float(a_value) = cell {
                if let ui::Cell::Float(b_value) = b[i] {
                    if !abs_diff_eq!(*a_value, b_value, epsilon = 0.001) {
                        return false;
                    }
                } else {
                    return false;
                }
            } else if *cell != b[i] {
                return false;
            }
        }
        true
    }

    fn vec_of_rows_equal(a: &[Row], b: &[Row]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        for (i, row) in a.iter().enumerate() {
            if row.chosen != b[i].chosen {
                return false;
            }
            if !vec_of_cells_equal(&row.cells, &b[i].cells) {
                return false;
            }
        }
        true
    }
}
