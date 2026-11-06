//! Chess in Roblox via the Pleco engine.
use pleco::bots::AlphaBetaSearcher;
use pleco::tools::Searcher;
use pleco::Board;

// make sure the runtime (random source) is linked
use luau_rt as _;

/// Number of leaf nodes `depth` plies from the start position (move-generator check).
#[unsafe(no_mangle)]
pub extern "C" fn perft(depth: u32) -> f64 {
    let board = Board::start_pos();
    pleco::board::perft::perft(&board, depth as u16) as f64
}

/// Play `moves` plies of self-play with alpha-beta at `depth`, printing each move.
#[unsafe(no_mangle)]
pub extern "C" fn selfplay(moves: u32, depth: u32) {
    let mut board = Board::start_pos();
    let mut line = Vec::new();
    for _ in 0..moves {
        if board.checkmate() || board.stalemate() {
            break;
        }
        let mv = AlphaBetaSearcher::best_move(board.shallow_clone(), depth as u16);
        line.push(mv.stringify());
        board.apply_move(mv);
    }
    println!("{}", line.join(" "));
    println!("{}", board.fen());
}
