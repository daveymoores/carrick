/**
 * Model interfaces for testing type bundling
 */

/**
 * Represents an order in the system
 */
export interface Order {
  /** Unique order identifier */
  id: string;
  /** Reference to the user who placed the order */
  userId: string;
  /** List of item IDs in the order */
  items: string[];
  /** Order status */
  status: OrderStatus;
  /** Total price in cents */
  totalCents: number;
  /** When the order was created */
  createdAt: Date;
}

/**
 * Order status enum
 */
export type OrderStatus = 'pending' | 'processing' | 'shipped' | 'delivered' | 'cancelled';

/**
 * Order item details
 */
export interface OrderItem {
  /** Item ID */
  id: string;
  /** Product name */
  name: string;
  /** Quantity ordered */
  quantity: number;
  /** Price per unit in cents */
  pricePerUnitCents: number;
}

/**
 * Order summary for list views
 */
export interface OrderSummary {
  id: string;
  status: OrderStatus;
  itemCount: number;
  totalCents: number;
}
