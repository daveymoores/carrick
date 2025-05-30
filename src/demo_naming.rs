use crate::analyzer::Analyzer;

pub fn demo_naming_strategy() {
    println!("=== Carrick Type Naming Strategy Demo ===\n");
    
    // Show the example naming strategy
    let example = Analyzer::example_naming_strategy();
    println!("{}\n", example);
    
    // Demonstrate different scenarios
    println!("=== Additional Examples ===\n");
    
    // Example 1: Multiple methods on same route
    let routes_and_methods = vec![
        ("/api/users", "GET"),
        ("/api/users", "POST"), 
        ("/api/users/:id", "GET"),
        ("/api/users/:id", "PUT"),
        ("/api/users/:id", "DELETE"),
    ];
    
    println!("1. Different methods on user routes:");
    for (route, method) in &routes_and_methods {
        let common_name = Analyzer::generate_common_type_alias_name(route, method, false);
        let unique_call1 = Analyzer::generate_unique_call_alias_name(route, method, false, 1);
        let unique_call2 = Analyzer::generate_unique_call_alias_name(route, method, false, 2);
        
        println!("   {} {} -> Common: {}", method, route, common_name);
        println!("             -> Calls: {}, {}", unique_call1, unique_call2);
    }
    
    println!("\n2. Environment variable based routes:");
    let env_routes = vec![
        ("${process.env.ORDER_SERVICE_URL}/orders", "GET"),
        ("${process.env.USER_SERVICE_URL}/users/:id", "GET"),
        ("${process.env.COMMENT_SERVICE_URL}/api/comments", "GET"),
    ];
    
    for (route, method) in &env_routes {
        let common_name = Analyzer::generate_common_type_alias_name(route, method, false);
        let unique_call = Analyzer::generate_unique_call_alias_name(route, method, false, 1);
        
        println!("   {} {} -> {}", method, route, common_name);
        println!("             -> Call: {}", unique_call);
    }
    
    println!("\n=== Type Comparison Workflow ===");
    println!("1. Producer (API endpoint) generates: GetApiCommentsResponse");
    println!("2. Consumer calls generate: GetApiCommentsResponse (same name!)");
    println!("3. ts-morph can compare: producer.isAssignableTo(consumer)");
    println!("4. Error reporting uses unique call IDs: GetApiCommentsResponseCall1, etc.");
    
    println!("\n=== Benefits ===");
    println!("✓ Semantic naming: Names reflect actual API structure");
    println!("✓ Type comparison: Common names enable ts-morph comparison");
    println!("✓ Error tracking: Unique call IDs for precise error reporting");
    println!("✓ Stable identifiers: Don't break on code formatting changes");
}