// VREF I/O standards, class II (outputs in banks 1 and 3 only, as ISE's DRC
// requires): an input and an output in each of the two banks. Checked
// against ISE bitgen only; ISE places the banks' VREF pins.
module iostdvref4 (input wire [1:0] i, output wire [1:0] o);
    assign o = {i[0] ^ i[1], ~i[1]};
endmodule
