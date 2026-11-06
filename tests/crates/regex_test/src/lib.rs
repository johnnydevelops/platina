use regex::Regex;

pub fn run() {
    let re = Regex::new(r"(?P<y>\d{4})-(?P<m>\d{2})-(?P<d>\d{2})").unwrap();
    let text = "dates: 2024-01-15, 1999-12-31 and 2026-11-06.";
    for c in re.captures_iter(text) {
        println!("{}/{}/{}", &c["d"], &c["m"], &c["y"]);
    }
    println!("{}", re.replace_all(text, "$d.$m.$y"));
    let words = Regex::new(r"\b\w+\b").unwrap();
    println!("{}", words.find_iter("the quick brown fox").count());
    println!("{}", Regex::new(r"^[a-z]+@[a-z]+\.(com|org)$").unwrap().is_match("ana@roblox.com"));
}
