use anyhow::{anyhow, bail, Context, Result};
use cloud::CloudClientInterface;
use cloud_openapi::models::{Database, ResourceLabel};

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

use crate::random_name::RandomNameGenerator;

use crate::commands::sqlite::database_has_link;

/// A user's selection of a database to link to a label
pub(super) enum DatabaseSelection {
    Existing(String),
    New(String),
    Cancelled,
}

/// Whether a database has already been linked or not
enum ExistingAppDatabaseSelection {
    NotYetLinked(DatabaseSelection),
    AlreadyLinked,
}

async fn get_database_selection_for_existing_app(
    name: &str,
    client: &impl CloudClientInterface,
    resource_label: &ResourceLabel,
    interact: &dyn InteractionStrategy,
) -> Result<ExistingAppDatabaseSelection> {
    let databases = client.get_databases(None).await?;
    if databases
        .iter()
        .any(|d| database_has_link(d, &resource_label.label, resource_label.app_name.as_deref()))
    {
        return Ok(ExistingAppDatabaseSelection::AlreadyLinked);
    }
    let selection = interact.prompt_database_selection(name, &resource_label.label, databases)?;
    Ok(ExistingAppDatabaseSelection::NotYetLinked(selection))
}

async fn get_database_selection_for_new_app(
    name: &str,
    client: &impl CloudClientInterface,
    label: &str,
    interact: &dyn InteractionStrategy,
) -> Result<DatabaseSelection> {
    let databases = client.get_databases(None).await?;
    interact.prompt_database_selection(name, label, databases)
}

pub(super) struct Interactive;

pub(super) trait InteractionStrategy {
    fn prompt_database_selection(
        &self,
        name: &str,
        label: &str,
        databases: Vec<Database>,
    ) -> Result<DatabaseSelection>;
}

impl InteractionStrategy for Interactive {
    fn prompt_database_selection(
        &self,
        name: &str,
        label: &str,
        databases: Vec<Database>,
    ) -> Result<DatabaseSelection> {
        let prompt = format!(
            r#"App "{name}" accesses a database labeled "{label}"
    Would you like to link an existing database or create a new database?"#
        );
        let existing_opt = "Use an existing database and link app to it";
        let create_opt = "Create a new database and link the app to it";
        let opts = vec![existing_opt, create_opt];
        let index = match dialoguer::Select::new()
            .with_prompt(prompt)
            .items(&opts)
            .default(1)
            .interact_opt()?
        {
            Some(i) => i,
            None => return Ok(DatabaseSelection::Cancelled),
        };
        match index {
            0 => self.prompt_for_existing_database(
                name,
                label,
                databases.into_iter().map(|d| d.name).collect::<Vec<_>>(),
            ),
            1 => self.prompt_link_to_new_database(
                name,
                label,
                databases
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect::<HashSet<_>>(),
            ),
            _ => bail!("Choose unavailable option"),
        }
    }
}

const NAME_GENERATION_MAX_ATTEMPTS: usize = 100;

impl Interactive {
    fn prompt_for_existing_database(
        &self,
        name: &str,
        label: &str,
        mut database_names: Vec<String>,
    ) -> Result<DatabaseSelection> {
        let prompt =
            format!(r#"Which database would you like to link to {name} using the label "{label}""#);
        let index = match dialoguer::Select::new()
            .with_prompt(prompt)
            .items(&database_names)
            .default(0)
            .interact_opt()?
        {
            Some(i) => i,
            None => return Ok(DatabaseSelection::Cancelled),
        };
        Ok(DatabaseSelection::Existing(database_names.remove(index)))
    }

    fn prompt_link_to_new_database(
        &self,
        name: &str,
        label: &str,
        existing_names: HashSet<&str>,
    ) -> Result<DatabaseSelection> {
        let generator = RandomNameGenerator::new();
        let default_name = generator
            .generate_unique(existing_names, NAME_GENERATION_MAX_ATTEMPTS)
            .context("could not generate unique database name")?;

        let prompt = format!(
            r#"What would you like to name your database?
    Note: This name is used when managing your database at the account level. The app "{name}" will refer to this database by the label "{label}".
    Other apps can use different labels to refer to the same database."#
        );
        let name = dialoguer::Input::new()
            .with_prompt(prompt)
            .default(default_name)
            .interact_text()?;
        Ok(DatabaseSelection::New(name))
    }
}

#[derive(Default)]
pub(super) struct Scripted {
    labels_to_dbs: HashMap<String, DatabaseRef>,
}

impl Scripted {
    pub(super) fn set_label_action(&mut self, label: &str, db: DatabaseRef) -> anyhow::Result<()> {
        match self.labels_to_dbs.entry(label.to_owned()) {
            Entry::Occupied(_) => bail!("Label {label} is linked more than once"),
            Entry::Vacant(e) => e.insert(db),
        };
        Ok(())
    }
}

// Using an enum to allow for future "any other db label" linking
#[derive(Clone, Debug, Default)]
pub(super) enum DefaultLabelAction {
    #[default]
    Reject,
}

// Using an enum to allow for future "create new and link that" linking
#[derive(Clone, Debug)]
pub(super) enum DatabaseRef {
    Named(String),
}

impl InteractionStrategy for Scripted {
    fn prompt_database_selection(
        &self,
        _name: &str,
        label: &str,
        databases: Vec<Database>,
    ) -> Result<DatabaseSelection> {
        let existing_names: HashSet<&str> = databases.iter().map(|db| db.name.as_str()).collect();
        let requested_db = self.db_ref_for(label)?;
        match requested_db {
            DatabaseRef::Named(requested_db) => {
                let name = requested_db.to_owned();
                if existing_names.contains(name.as_str()) {
                    Ok(DatabaseSelection::Existing(name))
                } else {
                    Ok(DatabaseSelection::New(name))
                }
            }
        }
    }
}

impl Scripted {
    fn db_ref_for(&self, label: &str) -> anyhow::Result<&DatabaseRef> {
        match self.labels_to_dbs.get(label) {
            Some(db_ref) => Ok(db_ref),
            None => Err(anyhow!("No link specified for label '{label}'")),
        }
    }
}

// Loops through an app's manifest and creates databases.
// Returns a list of database and label pairs that should be
// linked to the app once it is created.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_databases_for_new_app(
    client: &impl CloudClientInterface,
    name: &str,
    labels: HashSet<String>,
    interact: &dyn InteractionStrategy,
) -> anyhow::Result<Option<Vec<(String, String)>>> {
    let mut databases_to_link = Vec::new();
    for label in labels {
        let db = match get_database_selection_for_new_app(name, client, &label, interact).await? {
            DatabaseSelection::Existing(db) => db,
            DatabaseSelection::New(db) => {
                client.create_database(db.clone(), None).await?;
                db
            }
            // User canceled terminal interaction
            DatabaseSelection::Cancelled => return Ok(None),
        };
        databases_to_link.push((db, label));
    }
    Ok(Some(databases_to_link))
}

// Loops through an updated app's manifest and creates and links any newly referenced databases.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_and_link_databases_for_existing_app(
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: Uuid,
    labels: HashSet<String>,
    interact: &dyn InteractionStrategy,
) -> anyhow::Result<Option<()>> {
    for label in labels {
        let resource_label = ResourceLabel {
            app_id,
            label,
            app_name: Some(app_name.to_string()),
        };
        if let ExistingAppDatabaseSelection::NotYetLinked(selection) =
            get_database_selection_for_existing_app(app_name, client, &resource_label, interact)
                .await?
        {
            match selection {
                // User canceled terminal interaction
                DatabaseSelection::Cancelled => return Ok(None),
                DatabaseSelection::New(db) => {
                    client.create_database(db, Some(resource_label)).await?;
                }
                DatabaseSelection::Existing(db) => {
                    client
                        .create_database_link(&db, resource_label)
                        .await
                        .with_context(|| {
                            format!(r#"Could not link database "{}" to app "{}""#, db, app_name,)
                        })?;
                }
            }
        }
    }
    Ok(Some(()))
}

pub(super) async fn link_databases(
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: Uuid,
    database_labels: Vec<(String, String)>,
) -> anyhow::Result<()> {
    for (database, label) in database_labels {
        let resource_label = ResourceLabel {
            label,
            app_id,
            app_name: Some(app_name.to_owned()),
        };
        client
            .create_database_link(&database, resource_label)
            .await
            .with_context(|| {
                format!(
                    r#"Failed to link database "{}" to app "{}""#,
                    database, app_name
                )
            })?;
    }
    Ok(())
}
