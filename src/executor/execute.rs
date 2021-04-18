use {
    super::{
        alter::{create_table, drop},
        insert::insert,
        /*update::Update,*/
        query::query,
        types::{ComplexColumnName, Row as VecRow},
    },
    crate::{
        data::{get_name, Schema},
        parse_sql::Query,
        result::MutResult,
        store::{AlterTable, AutoIncrement, Store, StoreMut},
        MetaRecipe, PlannedRecipe, RecipeUtilities, Result, Row,
    },
    serde::Serialize,
    sqlparser::ast::{Assignment, ColumnDef, Statement},
    std::fmt::Debug,
    thiserror::Error as ThisError,
};

#[cfg(feature = "alter-table")]
use super::alter::alter_table;

#[derive(ThisError, Serialize, Debug, PartialEq)]
pub enum ExecuteError {
    #[error("query not supported")]
    QueryNotSupported,

    #[error("unsupported insert value type: {0}")]
    UnreachableUnsupportedInsertValueType(String),

    #[error("table does not exist")]
    TableNotExists,

    #[error("column could not be found")]
    ColumnNotFound,
}

#[derive(Serialize, Debug, PartialEq)]
pub enum Payload {
    Create,
    Insert(usize),
    Select {
        labels: Vec<String>,
        rows: Vec<Row>,
    },
    Delete(usize),
    Update(usize),
    DropTable,

    #[cfg(feature = "alter-table")]
    AlterTable,
}

pub async fn execute<
    Key: 'static + Debug,
    Storage: Store<Key> + StoreMut<Key> + AlterTable + AutoIncrement,
>(
    storage: Storage,
    statement: &Query,
) -> MutResult<Storage, Payload> {
    macro_rules! try_block {
        ($storage: expr, $block: block) => {{
            match (|| async { $block })().await {
                Err(e) => {
                    return Err(($storage, e));
                }
                Ok(v) => v,
            }
        }};
    }
    macro_rules! try_into {
        ($storage: expr, $expr: expr) => {
            match $expr {
                Err(e) => {
                    return Err(($storage, e));
                }
                Ok(v) => v,
            }
        };
    }

    let Query(statement) = statement;

    match statement {
        //- Modification
        //-- Tables
        Statement::CreateTable {
            name,
            columns,
            if_not_exists,
            ..
        } => create_table(storage, name, columns, *if_not_exists)
            .await
            .map(|(storage, _)| (storage, Payload::Create)),
        Statement::Drop {
            object_type,
            names,
            if_exists,
            ..
        } => drop(storage, object_type, names, *if_exists)
            .await
            .map(|(storage, _)| (storage, Payload::DropTable)),
        #[cfg(feature = "alter-table")]
        Statement::AlterTable { name, operation } => alter_table(storage, name, operation)
            .await
            .map(|(storage, _)| (storage, Payload::AlterTable)),

        //-- Rows
        Statement::Insert {
            table_name,
            columns,
            source,
            ..
        } => insert(storage, table_name, columns, source).await,
        Statement::Update {
            table_name,
            selection,
            assignments,
        } => {
            let rows = try_block!(storage, {
                let table_name = get_name(table_name)?;
                let Schema { column_defs, .. } = storage
                    .fetch_schema(table_name)
                    .await?
                    .ok_or(ExecuteError::TableNotExists)?;

                let columns = column_defs
                    .clone()
                    .into_iter()
                    .map(|column_def| {
                        let ColumnDef { name, .. } = column_def;
                        ComplexColumnName {
                            name: name.value,
                            table: (None, String::new()),
                        }
                    })
                    .collect();

                let filter = selection
                    .clone()
                    .map(|selection| PlannedRecipe::new(MetaRecipe::new(selection)?, &columns))
                    .unwrap_or(Ok(PlannedRecipe::TRUE))?;

                let assignments = assignments
                    .into_iter()
                    .map(|assignment| {
                        let Assignment { id, value } = assignment;
                        let column_compare = vec![id.value.clone()];
                        let index = columns
                            .iter()
                            .position(|column| column == &column_compare)
                            .ok_or(ExecuteError::ColumnNotFound)?;
                        let recipe = PlannedRecipe::new(MetaRecipe::new(value.clone())?, &columns)?;
                        Ok((index, recipe))
                    })
                    .collect::<Result<Vec<(usize, PlannedRecipe)>>>()?;

                let keyed_rows = storage
                    .scan_data(table_name)
                    .await?
                    .into_iter()
                    .filter_map(|row_result| {
                        let (key, row) = match row_result {
                            Ok(keyed_row) => keyed_row,
                            Err(error) => return Some(Err(error)),
                        };

                        let row = row.0;

                        let confirm_constraint = filter.confirm_constraint(&row.clone());
                        if let Ok(false) = confirm_constraint {
                            return None;
                        } else if let Err(error) = confirm_constraint {
                            return Some(Err(error));
                        }
                        let row = row
                            .iter()
                            .enumerate()
                            .map(|(index, old_value)| {
                                assignments
                                    .iter()
                                    .find(|(assignment_index, _)| assignment_index == &index)
                                    .map(|(_, assignment_recipe)| {
                                        assignment_recipe.clone().simplify_by_row(&row)?.confirm()
                                    })
                                    .unwrap_or(Ok(old_value.clone()))
                            })
                            .collect::<Result<VecRow>>();
                        Some(row.map(|row| (key, Row(row))))
                    })
                    .collect::<Result<Vec<(Key, Row)>>>()?;
                Ok(keyed_rows)
                /*let (keys, rows) = keyed_rows.into_iter().unzip(); // TODO: Improve
                let rows = validate(rows)?;
                Ok(keys.into_iter().zip(rows.into_iter().map(Row)).collect())*/
            });
            let num_rows = rows.len();
            storage
                .update_data(rows)
                .await
                .map(|(storage, _)| (storage, Payload::Update(num_rows)))
        }
        Statement::Delete {
            table_name,
            selection,
        } => {
            let keys = try_block!(storage, {
                let table_name = get_name(&table_name)?;
                let Schema { column_defs, .. } = storage
                    .fetch_schema(table_name)
                    .await?
                    .ok_or(ExecuteError::TableNotExists)?;

                let columns = column_defs
                    .clone()
                    .into_iter()
                    .map(|column_def| {
                        let ColumnDef { name, .. } = column_def;
                        ComplexColumnName {
                            name: name.value,
                            table: (None, String::new()),
                        }
                    })
                    .collect();
                let filter = selection
                    .clone()
                    .map(|selection| PlannedRecipe::new(MetaRecipe::new(selection)?, &columns))
                    .unwrap_or(Ok(PlannedRecipe::TRUE))?;

                storage
                    .scan_data(table_name)
                    .await?
                    .filter_map(|row_result| {
                        let (key, row) = match row_result {
                            Ok(keyed_row) => keyed_row,
                            Err(error) => return Some(Err(error)),
                        };

                        let row = row.0;

                        let confirm_constraint = filter.confirm_constraint(&row.clone());
                        match confirm_constraint {
                            Ok(true) => Some(Ok(key)),
                            Ok(false) => None,
                            Err(error) => Some(Err(error)),
                        }
                    })
                    .collect::<Result<Vec<Key>>>()
            });

            let num_keys = keys.len();

            storage
                .delete_data(keys)
                .await
                .map(|(storage, _)| (storage, Payload::Delete(num_keys)))
        }
        //- Selection
        Statement::Query(query_value) => {
            let result = try_into!(storage, query(&storage, *query_value.clone()).await);
            let (labels, rows) = result;
            let rows = rows.into_iter().map(Row).collect(); // I don't like this. TODO
            let payload = Payload::Select { labels, rows };
            Ok((storage, payload))
        }
        _ => Err((storage, ExecuteError::QueryNotSupported.into())),
    }
}
