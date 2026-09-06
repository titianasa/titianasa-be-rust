use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct ProgramSummary {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ListProgramsResponse {
    pub items: Vec<ProgramSummary>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProgramResponse {
    pub id: Uuid,
    pub status: String,
}

// GET /programs/{id}/tree — the modules attached to this program, in
// order, each carrying its own item outline. One join hop deeper than
// the old GET /curricula/{id}/tree, same fetch-flat/group-in-memory
// technique.
#[derive(Debug, serde::Serialize)]
pub struct ProgramModuleNode {
    pub module_id: Uuid,
    pub title: String,
    pub is_folder: bool,
    pub label_override: Option<String>,
    pub order_index: i32,
}

#[derive(Debug, serde::Serialize)]
pub struct ProgramTree {
    pub program: ProgramSummary,
    pub modules: Vec<ProgramModuleNode>,
}
