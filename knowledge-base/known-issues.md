# Known Issues

Problems seen in game that are parked for later.

## Unit info box text at the old position

With the `fit` UI layout, the text of the unit info popup is still drawn at the left, where the box used to sit in the stretched layout, even though the box itself moved. Probably the help popup's draw callback or code that still uses the old control bar multipliers.
