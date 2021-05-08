use {
	super::{auto_increment, columns_to_positions, validate, validate_unique},
	crate::{
		data::{get_name, Schema},
		executor::query::query,
		ExecuteError, Payload, Result, Row, StorageInner,
	},
	sqlparser::ast::{Ident, ObjectName, Query},
};

pub async fn insert(
	mut storages: Vec<(String, &mut StorageInner)>,
	table_name: &ObjectName,
	columns: &Vec<Ident>,
	source: &Box<Query>,
) -> Result<Payload> {
	let table_name = get_name(table_name)?;
	let Schema { column_defs, .. } = storages[0]
		.1
		.fetch_schema(table_name)
		.await?
		.ok_or(ExecuteError::TableNotExists)?;

	// TODO: Multi storage
	let (_, rows) = query(&storages, *source.clone()).await?;
	let column_positions = columns_to_positions(&column_defs, columns)?;

	let rows = validate(&column_defs, &column_positions, rows)?;
	#[cfg(feature = "auto-increment")]
	let rows = auto_increment(storages[0].1, table_name, &column_defs, rows).await?;
	validate_unique(storages[0].1, table_name, &column_defs, &rows, None).await?;
	let rows: Vec<Row> = rows.into_iter().map(Row).collect();

	let num_rows = rows.len();

	storages[0]
		.1
		.insert_data(table_name, rows)
		.await
		.map(|_| Payload::Insert(num_rows))
}