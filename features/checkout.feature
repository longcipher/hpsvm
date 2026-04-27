Feature: Checkout
  As a shopper
  I want to checkout my cart
  So that an order is created from the selected items

  Scenario: Checking out a cart with multiple items
    Given the cart contains "Tea" priced at 450 cents with quantity 2
    And the cart contains "Cake" priced at 350 cents with quantity 1
    When the customer checks out
    Then an order should be created with total 1250 cents
    And the cart should be empty
