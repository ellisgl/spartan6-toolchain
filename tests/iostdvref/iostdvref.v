// VREF I/O standards, one per bank: an input (compared against the bank's
// VREF) and an output each. Checked against ISE bitgen only (the Mojo V3's
// banks run at 3.3 V); ISE places the banks' VREF pins.
module iostdvref (input wire [3:0] i, output wire [3:0] o);
    assign o = {i[0] ^ i[1], i[1] & i[2], i[2] | i[3], ~i[3] ^ i[0]};
endmodule
