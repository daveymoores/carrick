// Arrow function export (what you already had)
export const handleProductGet = (req, res) => {
  res.json([
    { id: 1, name: "Product 1" },
    { id: 2, name: "Product 2" },
  ]);
};

// Function expression export
export const handleProductPost = function (req, res) {
  res.json({
    success: true,
    message: "Product created successfully",
    product: {
      id: 3,
      name: req.body.name || "New Product",
    },
  });
};

// Function declaration export
export function handleProductDelete(req, res) {
  const productId = req.params.id;
  res.json({
    success: true,
    message: `Product ${productId} deleted successfully`,
  });
}

// Default export (arrow function)
export default (req, res) => {
  res.json({
    apiVersion: "1.0",
    endpoints: [
      { path: "/products", methods: ["GET", "POST"] },
      { path: "/products/:id", methods: ["GET", "DELETE"] },
    ],
  });
};

// TODO - Classes not yet covered (may never be covered for MVP)
// Class with methods (for testing class method handling)
export class ProductController {
  // Static method that could be used as a route handler
  static listAll(req, res) {
    res.json({
      products: [
        { id: 1, name: "Product 1", price: 19.99 },
        { id: 2, name: "Product 2", price: 29.99 },
        { id: 3, name: "Product 3", price: 39.99 },
      ],
      count: 3,
    });
  }

  // Instance method example
  getProductDetails(req, res) {
    const id = req.params.id;
    res.json({
      id: parseInt(id),
      name: `Product ${id}`,
      description: "Detailed product description",
      specifications: {
        weight: "1.2kg",
        dimensions: "10 × 20 × 5 cm",
      },
    });
  }
}

// Multiple declarations in one statement (to test variable declarations with multiple bindings)
export const handleProductUpdate = (req, res) => {
    res.json({ status: "updated" });
  },
  getProductStats = function (req, res) {
    res.json({
      totalProducts: 100,
      avgPrice: 29.99,
      categories: ["Electronics", "Clothing", "Books"],
    });
  };

// Named function expression (slightly different case)
export const handleInventory = function checkInventory(req, res) {
  res.json({
    inStock: true,
    quantity: 42,
    warehouse: "Main",
  });
};
