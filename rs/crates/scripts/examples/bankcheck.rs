fn main() {
    let account = std::env::args().nth(1).unwrap_or_else(|| "admin".to_string());
    let save = lifescape_sdk::save::load_account(&account, None).expect("no save");
    let mut bank: Vec<_> = save.bank.iter().collect();
    bank.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    println!("bank of {account}: {} stacks", bank.len());
    for (id, count) in bank.iter().take(30) {
        let name = lifescape_gamedata::obj_display_name(**id as usize).map(str::to_string).unwrap_or_else(|| format!("#{id}"));
        println!("{id} x{count} {name}");
    }
    println!("--- worn:");
    for (id, count) in &save.worn {
        let name = lifescape_gamedata::obj_display_name(*id as usize).map(str::to_string).unwrap_or_else(|| format!("#{id}"));
        println!("{id} x{count} {name}");
    }
}
