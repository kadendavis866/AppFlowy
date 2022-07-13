use crate::grid::block_test::script::GridRowTest;
use crate::grid::block_test::script::RowScript::*;
use flowy_grid::entities::FieldType;
use flowy_grid_data_model::revision::RowMetaChangeset;

#[tokio::test]
async fn grid_create_row_count_test() {
    let mut test = GridRowTest::new().await;
    let scripts = vec![
        AssertRowCount(3),
        CreateEmptyRow,
        CreateEmptyRow,
        CreateRow {
            row_rev: test.row_builder().build(),
        },
        AssertRowCount(6),
    ];
    test.run_scripts(scripts).await;
}

#[tokio::test]
async fn grid_update_row() {
    let mut test = GridRowTest::new().await;
    let row_rev = test.row_builder().build();
    let changeset = RowMetaChangeset {
        row_id: row_rev.id.clone(),
        height: None,
        visibility: None,
        cell_by_field_id: Default::default(),
    };

    let scripts = vec![AssertRowCount(3), CreateRow { row_rev }, UpdateRow { changeset }];
    test.run_scripts(scripts).await;

    let expected_row = test.last_row().unwrap();
    let scripts = vec![AssertRow { expected_row }, AssertRowCount(4)];
    test.run_scripts(scripts).await;
}

#[tokio::test]
async fn grid_delete_row() {
    let mut test = GridRowTest::new().await;
    let row_1 = test.row_builder().build();
    let row_2 = test.row_builder().build();
    let row_ids = vec![row_1.id.clone(), row_2.id.clone()];
    let scripts = vec![
        AssertRowCount(3),
        CreateRow { row_rev: row_1 },
        CreateRow { row_rev: row_2 },
        AssertBlockCount(1),
        AssertBlock {
            block_index: 0,
            row_count: 5,
            start_row_index: 0,
        },
        DeleteRows { row_ids },
        AssertBlock {
            block_index: 0,
            row_count: 3,
            start_row_index: 0,
        },
    ];
    test.run_scripts(scripts).await;
}

#[tokio::test]
async fn grid_row_add_cells_test() {
    let mut test = GridRowTest::new().await;
    let mut builder = test.row_builder();

    let text_field_id = builder.insert_text_cell("hello world");
    let number_field_id = builder.insert_number_cell("18,443");
    let date_field_id = builder.insert_date_cell("1647251762");
    let single_select_field_id = builder.insert_single_select_cell(|options| options.first().unwrap());
    builder.insert_multi_select_cell(|options| options);
    builder.insert_checkbox_cell("false");
    let url_field_id = builder.insert_url_cell("https://appflowy.io");

    let row_rev = builder.build();
    let row_id = row_rev.id.clone();
    let scripts = vec![
        CreateRow { row_rev },
        AssertCell {
            row_id: row_id.clone(),
            field_id: text_field_id,
            field_type: FieldType::RichText,
            expected: "hello world".to_owned(),
        },
        AssertCell {
            row_id: row_id.clone(),
            field_id: number_field_id,
            field_type: FieldType::Number,
            expected: "$18,443.00".to_owned(),
        },
        AssertCell {
            row_id: row_id.clone(),
            field_id: single_select_field_id,
            field_type: FieldType::SingleSelect,
            expected: "Completed".to_owned(),
        },
        AssertCell {
            row_id: row_id.clone(),
            field_id: date_field_id,
            field_type: FieldType::DateTime,
            expected: "2022/03/14".to_owned(),
        },
        AssertCell {
            row_id: row_id.clone(),
            field_id: url_field_id,
            field_type: FieldType::URL,
            expected: "https://appflowy.io/".to_owned(),
        },
    ];
    test.run_scripts(scripts).await;
}
