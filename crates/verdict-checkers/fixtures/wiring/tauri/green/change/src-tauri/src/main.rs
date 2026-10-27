#[tauri::command]
fn save_note(text: String) -> Result<(), String> {
    std::fs::write("note.txt", text).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![save_note])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
